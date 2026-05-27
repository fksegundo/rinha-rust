pub fn warmup_queries() -> usize {
    std::env::var("RINHA_WARMUP_QUERIES")
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(256)
}

pub fn payload_warmup_requests() -> usize {
    std::env::var("RINHA_PAYLOAD_WARMUP_REQUESTS")
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(256)
}
