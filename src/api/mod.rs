use crate::http;
use crate::index::SpecialistIndex;
use crate::runtime;
use crate::vector;
use crate::{PACKED_DIMS, SCALE};
use std::net::TcpListener;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

pub fn run(index_path: &str, bind_addr: &str, fd_socket: Option<&str>) {
    if std::env::var("RINHA_MLOCK_ALL").as_deref() == Ok("1") {
        mlock_current_and_future();
    }

    let index = Arc::new(
        SpecialistIndex::open(index_path)
            .unwrap_or_else(|e| panic!("failed to open index '{}': {}", index_path, e)),
    );

    if std::env::var("RINHA_MLOCK_INDEX").as_deref() != Ok("0") {
        index.mlock_all();
    }
    if std::env::var("RINHA_PRETOUCH_INDEX").as_deref() != Ok("0") {
        index.pretouch_all();
    }

    let ready = Arc::new(AtomicBool::new(false));

    eprintln!(
        "warming up index with {} queries...",
        runtime::warmup_queries()
    );
    warm_up_index(&index);
    eprintln!(
        "warming up payload path with {} requests...",
        runtime::payload_warmup_requests()
    );
    warm_up_payload_path(&index);
    ready.store(true, Ordering::Release);
    eprintln!("warmup complete, accepting connections");

    let pool_size = thread_pool_size();

    if let Some(socket_path) = fd_socket {
        run_fd_mode(index, ready, socket_path, pool_size);
    } else {
        run_tcp_mode(index, ready, bind_addr, pool_size);
    }
}

fn thread_pool_size() -> usize {
    std::env::var("RINHA_THREAD_POOL_SIZE")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(56)
}

fn fd_evented_enabled() -> bool {
    std::env::var("RINHA_FD_EVENTED")
        .ok()
        .map(|v| v != "0")
        .unwrap_or(true)
}

fn run_fd_mode(
    index: Arc<SpecialistIndex>,
    ready: Arc<AtomicBool>,
    socket_path: &str,
    pool_size: usize,
) {
    use crate::fd_passing;
    if fd_evented_enabled() {
        eprintln!("starting FD evented server on {}", socket_path);
        fd_passing::run_fd_evented_server(socket_path, move |req| {
            handle_request(req, &index, &ready)
        });
    } else {
        fd_passing::run_fd_server(socket_path, pool_size, move |stream| {
            let index = Arc::clone(&index);
            let ready = Arc::clone(&ready);
            http::handle_connection(stream, |req| handle_request(req, &index, &ready));
        });
    }
}

fn run_tcp_mode(
    index: Arc<SpecialistIndex>,
    ready: Arc<AtomicBool>,
    bind_addr: &str,
    pool_size: usize,
) {
    let listener = TcpListener::bind(bind_addr)
        .unwrap_or_else(|e| panic!("failed to bind {}: {}", bind_addr, e));

    let pool = threadpool::Builder::new()
        .num_threads(pool_size)
        .thread_stack_size(64 * 1024)
        .build();

    for stream in listener.incoming() {
        match stream {
            Ok(stream) => {
                let _ = stream.set_nodelay(true);
                let index = Arc::clone(&index);
                let ready = Arc::clone(&ready);
                pool.execute(move || {
                    http::handle_connection(stream, |req| handle_request(req, &index, &ready));
                });
            }
            Err(e) => {
                eprintln!("accept error: {}", e);
            }
        }
    }
}

fn mlock_current_and_future() {
    #[cfg(target_os = "linux")]
    unsafe {
        let mode = std::env::var("RINHA_MLOCK_ALL_MODE").unwrap_or_else(|_| "future".to_string());
        let flags = match mode.as_str() {
            "current" => libc::MCL_CURRENT,
            "current-future" => libc::MCL_CURRENT | libc::MCL_FUTURE,
            "future" => libc::MCL_FUTURE,
            other => {
                eprintln!("invalid RINHA_MLOCK_ALL_MODE='{}'; using MCL_FUTURE", other);
                libc::MCL_FUTURE
            }
        };
        if libc::mlockall(flags) != 0 {
            eprintln!(
                "mlockall({}) failed: {}",
                mode,
                std::io::Error::last_os_error()
            );
        } else {
            eprintln!("mlockall({}) succeeded", mode);
        }
    }

    #[cfg(not(target_os = "linux"))]
    {
        eprintln!("mlockall is only supported on linux");
    }
}

fn warm_up_index(index: &SpecialistIndex) {
    let count = runtime::warmup_queries();
    let scale = SCALE as usize;
    for i in 0..count {
        let mut query = [0i16; PACKED_DIMS];
        for (dim, value) in query.iter_mut().enumerate() {
            let raw = ((i * 313 + dim * 1009) % (scale + 1)) as i16;
            *value = if (dim == 5 || dim == 6) && i % 4 == 0 {
                -(SCALE as i16)
            } else {
                raw
            };
        }
        let _ = index.predict_fraud_count(&query);
    }
}

fn warm_up_payload_path(index: &SpecialistIndex) {
    let count = runtime::payload_warmup_requests();
    if count == 0 {
        return;
    }

    let ready = AtomicBool::new(true);
    for i in 0..count {
        let body = WARMUP_PAYLOADS[i % WARMUP_PAYLOADS.len()];
        let mut request = Vec::with_capacity(body.len() + 96);
        warm_up_payload_body(index, &ready, body, &mut request);
    }
}

fn warm_up_payload_body(
    index: &SpecialistIndex,
    ready: &AtomicBool,
    body: &[u8],
    request: &mut Vec<u8>,
) {
    request.clear();
    request.extend_from_slice(b"POST /fraud-score HTTP/1.1\r\nHost: localhost\r\nContent-Length: ");
    request.extend_from_slice(body.len().to_string().as_bytes());
    request.extend_from_slice(b"\r\n\r\n");
    request.extend_from_slice(body);

    if let Some((req, _)) = http::parse_request(request) {
        let _ = handle_request(&req, index, ready);
    }
}

const WARMUP_PAYLOADS: &[&[u8]] = &[
    br#"{"id":"warmup-1","transaction":{"amount":441.59,"installments":1,"requested_at":"2027-07-09T16:31:06Z"},"customer":{"avg_amount":883.18,"tx_count_24h":1,"known_merchants":["MERC-004","MERC-017"]},"merchant":{"id":"MERC-004","mcc":"5411","avg_amount":302.78},"terminal":{"is_online":false,"card_present":true,"km_from_home":33.88},"last_transaction":{"timestamp":"2027-06-04T14:14:22Z","km_from_current":18.43}}"#,
    br#"{"id":"warmup-2","transaction":{"amount":5293.06,"installments":8,"requested_at":"2028-09-19T03:34:29Z"},"customer":{"avg_amount":60.14,"tx_count_24h":11,"known_merchants":["MERC-009","MERC-001"]},"merchant":{"id":"MERC-087","mcc":"7995","avg_amount":21.57},"terminal":{"is_online":false,"card_present":false,"km_from_home":265.78},"last_transaction":{"timestamp":"2024-01-04T03:43:32Z","km_from_current":722.93}}"#,
    br#"{"id":"warmup-3","transaction":{"amount":7318.26,"installments":8,"requested_at":"2028-07-05T03:41:22Z"},"customer":{"avg_amount":158.57,"tx_count_24h":11,"known_merchants":["MERC-013","MERC-010"]},"merchant":{"id":"MERC-073","mcc":"7801","avg_amount":37.46},"terminal":{"is_online":true,"card_present":false,"km_from_home":417.33},"last_transaction":null}"#,
    br#"{"customer":{"avg_amount":68.88,"tx_count_24h":18,"known_merchants":["MERC-004","MERC-015","MERC-007"]},"id":"warmup-4","last_transaction":{"timestamp":"2026-03-17T01:58:06Z","km_from_current":660.92},"merchant":{"id":"MERC-062","mcc":"7801","avg_amount":25.55},"terminal":{"is_online":true,"card_present":false,"km_from_home":881.61},"transaction":{"amount":4368.82,"installments":8,"requested_at":"2026-03-17T02:04:06Z"}}"#,
    br#"{"id":"warmup-5","transaction":{"amount":29.47,"installments":2,"requested_at":"2028-12-24T08:34:05Z"},"customer":{"avg_amount":58.94,"tx_count_24h":3,"known_merchants":["MERC-004","MERC-014"]},"merchant":{"id":"MERC-014","mcc":"5411","avg_amount":378.62},"terminal":{"is_online":false,"card_present":true,"km_from_home":20.36},"last_transaction":{"timestamp":"2027-11-28T15:22:55Z","km_from_current":16.71}}"#,
    br#"{"id":"warmup-6","transaction":{"amount":9797.7,"installments":7,"requested_at":"2026-11-14T06:09:00Z"},"customer":{"avg_amount":99.49,"tx_count_24h":13,"known_merchants":["MERC-006","MERC-014","MERC-013"]},"merchant":{"id":"MERC-094","mcc":"7802","avg_amount":33.01},"terminal":{"is_online":false,"card_present":true,"km_from_home":396.12},"last_transaction":{"timestamp":"2026-03-18T15:14:27Z","km_from_current":712.42}}"#,
];

fn is_ready(ready: &AtomicBool) -> bool {
    ready.load(Ordering::Acquire)
}

fn handle_request(
    req: &http::Request,
    index: &SpecialistIndex,
    ready: &AtomicBool,
) -> &'static [u8] {
    match req.method {
        http::Method::Get if req.path == b"/ready" => {
            if is_ready(ready) {
                http::RESPONSE_READY
            } else {
                http::RESPONSE_NOT_READY
            }
        }
        http::Method::Post if req.path == b"/fraud-score" => {
            if !is_ready(ready) {
                return http::RESPONSE_NOT_READY;
            }
            let mut query = [0i16; 16];
            match vector::parse_query(req.body, &mut query) {
                Ok(()) => {
                    let count = index.predict_fraud_count(&query) as usize;
                    if count < http::FRAUD_RESPONSES.len() {
                        http::FRAUD_RESPONSES[count]
                    } else {
                        http::FRAUD_RESPONSES[5]
                    }
                }
                Err(_) => http::RESPONSE_BAD_REQUEST,
            }
        }
        _ => http::RESPONSE_NOT_FOUND,
    }
}
