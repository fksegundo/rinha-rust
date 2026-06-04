fn main() {
    let index_path = std::env::var("RINHA_INDEX_PATH")
        .unwrap_or_else(|_| "/app/index/rinha-specialist.idx".to_string());
    let fd_socket = std::env::var("RINHA_FD_SOCKET").ok().or_else(|| {
        let hostname = std::env::var("HOSTNAME").unwrap_or_else(|_| "api".to_string());
        Some(format!("/sockets/{}.sock", hostname))
    });

    rinha_rust::api::run(&index_path, fd_socket.as_deref());
}
