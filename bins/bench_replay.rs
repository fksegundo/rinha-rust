use std::time::Instant;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 3 {
        eprintln!("usage: {} <index_path> <test_data.json> [max_entries]", args[0]);
        std::process::exit(1);
    }

    let index_path = &args[1];
    let data_path = &args[2];
    let max_entries: usize = args
        .get(3)
        .and_then(|s| s.parse().ok())
        .unwrap_or(usize::MAX);

    let index = rinha_rust::index::SpecialistIndex::open(index_path)
        .unwrap_or_else(|e| panic!("failed to open index '{}': {}", index_path, e));

    let metadata = index.metadata();
    eprintln!(
        "index: {} references, {} partitions, {} nodes, {} blocks",
        metadata.reference_count,
        metadata.partition_count,
        metadata.node_count,
        metadata.block_count
    );

    let raw = std::fs::read(data_path)
        .unwrap_or_else(|e| panic!("failed to read '{}': {}", data_path, e));

    let parsed: serde_json::Value =
        serde_json::from_slice(&raw).expect("invalid test-data.json");
    let entries = parsed["entries"]
        .as_array()
        .expect("missing 'entries' array");

    let total = entries.len().min(max_entries);
    eprintln!("benchmarking {} entries", total);

    let mut parse_ns = Vec::with_capacity(total);
    let mut predict_ns = Vec::with_capacity(total);
    let mut total_blocks = 0u64;
    let mut total_leaves = 0u64;

    for (i, entry) in entries.iter().take(total).enumerate() {
        let request = &entry["request"];
        let payload = serde_json::to_vec(request).expect("serialize request");

        let mut query = [0i16; 16];

        let t0 = Instant::now();
        let _ = rinha_rust::vector::parse_query(&payload, &mut query);
        let t1 = Instant::now();

        let (count, stats) = index.predict_fraud_count_with_stats(&query);
        let t2 = Instant::now();

        parse_ns.push((t1 - t0).as_nanos() as u64);
        predict_ns.push((t2 - t1).as_nanos() as u64);
        total_blocks += stats.blocks_scanned as u64;
        total_leaves += stats.leaves_scanned as u64;

        if (i + 1) % 5000 == 0 {
            eprintln!("  {} / {}", i + 1, total);
        }
        let _ = count;
    }

    parse_ns.sort_unstable();
    predict_ns.sort_unstable();

    let p50_idx = |v: &[u64]| v[v.len() * 50 / 100];
    let p90_idx = |v: &[u64]| v[v.len() * 90 / 100];
    let p95_idx = |v: &[u64]| v[v.len() * 95 / 100];
    let p99_idx = |v: &[u64]| v[(v.len() - 1).min(v.len() * 99 / 100)];
    let p999_idx = |v: &[u64]| v[(v.len() - 1).min(v.len() * 999 / 1000)];

    let avg_parse = parse_ns.iter().sum::<u64>() as f64 / parse_ns.len() as f64;
    let avg_predict = predict_ns.iter().sum::<u64>() as f64 / predict_ns.len() as f64;
    let avg_blocks = total_blocks as f64 / total as f64;
    let avg_leaves = total_leaves as f64 / total as f64;

    println!("{{");
    println!("  \"entries\": {},", total);
    println!("  \"parse_query_us\": {{");
    println!("    \"p50\": {:.3},", p50_idx(&parse_ns) as f64 / 1000.0);
    println!("    \"p90\": {:.3},", p90_idx(&parse_ns) as f64 / 1000.0);
    println!("    \"p95\": {:.3},", p95_idx(&parse_ns) as f64 / 1000.0);
    println!("    \"p99\": {:.3},", p99_idx(&parse_ns) as f64 / 1000.0);
    println!("    \"p99.9\": {:.3},", p999_idx(&parse_ns) as f64 / 1000.0);
    println!("    \"avg\": {:.3},", avg_parse / 1000.0);
    println!("    \"max\": {:.3}", parse_ns.last().copied().unwrap_or(0) as f64 / 1000.0);
    println!("  }},");
    println!("  \"predict_fraud_count_us\": {{");
    println!("    \"p50\": {:.3},", p50_idx(&predict_ns) as f64 / 1000.0);
    println!("    \"p90\": {:.3},", p90_idx(&predict_ns) as f64 / 1000.0);
    println!("    \"p95\": {:.3},", p95_idx(&predict_ns) as f64 / 1000.0);
    println!("    \"p99\": {:.3},", p99_idx(&predict_ns) as f64 / 1000.0);
    println!("    \"p99.9\": {:.3},", p999_idx(&predict_ns) as f64 / 1000.0);
    println!("    \"avg\": {:.3},", avg_predict / 1000.0);
    println!("    \"max\": {:.3}", predict_ns.last().copied().unwrap_or(0) as f64 / 1000.0);
    println!("  }},");
    println!("  \"combined_us\": {{");
    let mut combined: Vec<u64> = parse_ns
        .iter()
        .zip(predict_ns.iter())
        .map(|(a, b)| a + b)
        .collect();
    combined.sort_unstable();
    println!("    \"p50\": {:.3},", p50_idx(&combined) as f64 / 1000.0);
    println!("    \"p90\": {:.3},", p90_idx(&combined) as f64 / 1000.0);
    println!("    \"p95\": {:.3},", p95_idx(&combined) as f64 / 1000.0);
    println!("    \"p99\": {:.3},", p99_idx(&combined) as f64 / 1000.0);
    println!("    \"p99.9\": {:.3},", p999_idx(&combined) as f64 / 1000.0);
    println!("    \"avg\": {:.3},", (avg_parse + avg_predict) / 1000.0);
    println!("    \"max\": {:.3}", combined.last().copied().unwrap_or(0) as f64 / 1000.0);
    println!("  }},");
    println!("  \"search_stats\": {{");
    println!("    \"avg_blocks_scanned\": {:.1},", avg_blocks);
    println!("    \"avg_leaves_scanned\": {:.1}", avg_leaves);
    println!("  }}");
    println!("}}");
}
