pub fn warmup_queries() -> usize {
    std::env::var("RINHA_WARMUP_QUERIES")
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(256)
}
