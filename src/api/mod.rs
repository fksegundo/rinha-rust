use crate::http;
use crate::index::SpecialistIndex;
use crate::vector;
use std::net::TcpListener;
use std::sync::Arc;

pub fn run(index_path: &str, bind_addr: &str, fd_socket: Option<&str>) {
    let index = Arc::new(
        SpecialistIndex::open(index_path)
            .unwrap_or_else(|e| panic!("failed to open index '{}': {}", index_path, e)),
    );

    // Warm up
    {
        let warm_query = [0i16; 16];
        let _ = index.predict_fraud_count(&warm_query);
    }

    if let Some(socket_path) = fd_socket {
        run_fd_mode(index, socket_path);
    } else {
        run_tcp_mode(index, bind_addr);
    }
}

fn run_fd_mode(index: Arc<SpecialistIndex>, socket_path: &str) {
    use crate::fd_passing;
    fd_passing::run_fd_server(socket_path, move |stream| {
        let index = Arc::clone(&index);
        http::handle_connection(stream, |req| handle_request(req, &index));
    });
}

fn run_tcp_mode(index: Arc<SpecialistIndex>, bind_addr: &str) {
    let listener = TcpListener::bind(bind_addr)
        .unwrap_or_else(|e| panic!("failed to bind {}: {}", bind_addr, e));

    let num_threads = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4);
    let pool = threadpool::ThreadPool::new(num_threads);

    for stream in listener.incoming() {
        match stream {
            Ok(stream) => {
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
