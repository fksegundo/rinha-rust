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

    if std::env::var("RINHA_MLOCK_INDEX").as_deref() == Ok("1") {
        index.mlock_all();
    }

    let ready = Arc::new(AtomicBool::new(false));
    spawn_warmup(Arc::clone(&index), Arc::clone(&ready));

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
        .unwrap_or(256)
}

fn run_fd_mode(
    index: Arc<SpecialistIndex>,
    ready: Arc<AtomicBool>,
    socket_path: &str,
    pool_size: usize,
) {
    use crate::fd_passing;
    fd_passing::run_fd_server(socket_path, pool_size, move |stream| {
        let index = Arc::clone(&index);
        let ready = Arc::clone(&ready);
        http::handle_connection(stream, |req| handle_request(req, &index, &ready));
    });
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

fn spawn_warmup(index: Arc<SpecialistIndex>, ready: Arc<AtomicBool>) {
    std::thread::spawn(move || {
        warm_up_index(&index);
        ready.store(true, Ordering::Release);
    });
}

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
