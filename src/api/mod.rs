use crate::http;
use crate::index::SpecialistIndex;
use crate::runtime;
use crate::vector;
use crate::{PACKED_DIMS, SCALE};
use std::net::TcpListener;
use std::sync::Arc;

pub fn run(index_path: &str, bind_addr: &str, fd_socket: Option<&str>) {
    let index = Arc::new(
        SpecialistIndex::open(index_path)
            .unwrap_or_else(|e| panic!("failed to open index '{}': {}", index_path, e)),
    );

    if std::env::var("RINHA_MLOCK_INDEX").as_deref() == Ok("1") {
        index.mlock_all();
    }

    warm_up_index(&index);

    let pool_size = thread_pool_size();

    if let Some(socket_path) = fd_socket {
        run_fd_mode(index, socket_path, pool_size);
    } else {
        run_tcp_mode(index, bind_addr, pool_size);
    }
}

fn thread_pool_size() -> usize {
    std::env::var("RINHA_THREAD_POOL_SIZE")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(512)
}

fn run_fd_mode(index: Arc<SpecialistIndex>, socket_path: &str, pool_size: usize) {
    use crate::fd_passing;
    fd_passing::run_fd_server(socket_path, pool_size, move |stream| {
        let index = Arc::clone(&index);
        http::handle_connection(stream, |req| handle_request(req, &index));
    });
}

fn run_tcp_mode(index: Arc<SpecialistIndex>, bind_addr: &str, pool_size: usize) {
    let listener = TcpListener::bind(bind_addr)
        .unwrap_or_else(|e| panic!("failed to bind {}: {}", bind_addr, e));

    let pool = threadpool::Builder::new()
        .num_threads(pool_size)
        .thread_stack_size(256 * 1024)
        .build();

    for stream in listener.incoming() {
        match stream {
            Ok(stream) => {
                let _ = stream.set_nodelay(true);
                let index = Arc::clone(&index);
                pool.execute(move || {
                    http::handle_connection(stream, |req| handle_request(req, &index));
                });
            }
            Err(e) => {
                eprintln!("accept error: {}", e);
            }
        }
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

fn handle_request(req: &http::Request, index: &SpecialistIndex) -> &'static [u8] {
    match req.method {
        http::Method::Get if req.path == b"/ready" => http::RESPONSE_READY,
        http::Method::Post if req.path == b"/fraud-score" => {
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
                Err(_) => {
                    // For valid challenge payloads this should never happen
                    http::RESPONSE_FRAUD_0
                }
            }
        }
        _ => b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\n\r\n",
    }
}
