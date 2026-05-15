use std::env;

fn main() {
    let args: Vec<String> = env::args().collect();
    if args.len() < 3 {
        eprintln!(
            "Usage: {} <index.idx> <test-data.json> [limit] [--exact] [--diag]",
            args[0]
        );
        std::process::exit(1);
    }

    let index_path = &args[1];
    let test_data_path = &args[2];
    let limit = args.iter().skip(3).find_map(|s| {
        (!s.starts_with("--"))
            .then(|| s.parse::<usize>().ok())
            .flatten()
    });
    let check_exact = args.iter().any(|s| s == "--exact");
    let diag = args.iter().any(|s| s == "--diag");

    eprintln!("Loading index...");
    let index = rinha_rust::index::SpecialistIndex::open(index_path)
        .unwrap_or_else(|e| panic!("failed to open index: {}", e));

    eprintln!("Loading test data...");
    let test_data = std::fs::read_to_string(test_data_path)
        .unwrap_or_else(|e| panic!("failed to read test data: {}", e));

    let test_json: serde_json::Value = serde_json::from_str(&test_data)
        .unwrap_or_else(|e| panic!("failed to parse test data: {}", e));

    let entries = test_json["entries"]
        .as_array()
        .unwrap_or_else(|| panic!("missing entries array"));

    let mut total = 0usize;
    let mut mismatches = 0usize;
    let mut partitions_visited = Vec::new();
    let mut nodes_visited = Vec::new();
    let mut leaves_scanned = Vec::new();
    let mut blocks_scanned = Vec::new();

    for (idx, entry) in entries.iter().enumerate() {
        if let Some(l) = limit {
            if idx >= l {
                break;
            }
        }

        let request = serde_json::to_vec(&entry["request"]).unwrap();

        let mut query = [0i16; 16];
        if rinha_rust::vector::parse_query(&request, &mut query).is_err() {
            eprintln!("WARN: failed to parse request");
            continue;
        }

        let specialist_count = if diag {
            let (count, stats) = index.predict_fraud_count_with_stats(&query);
            partitions_visited.push(stats.partitions_visited);
            nodes_visited.push(stats.nodes_visited);
            leaves_scanned.push(stats.leaves_scanned);
            blocks_scanned.push(stats.blocks_scanned);
            count
        } else {
            index.predict_fraud_count(&query)
        };
        let exact_count = if check_exact {
            Some(index.predict_fraud_count_exact(&query))
        } else {
            None
        };
        let expected_count = entry["expected_fraud_score"]
            .as_f64()
            .map(|score| (score * 5.0).round() as u8);

        if let Some(expected_count) = expected_count {
            if expected_count != specialist_count {
                mismatches += 1;
                if mismatches <= 10 {
                    eprintln!(
                        "MISMATCH idx={}, expected_count={}, specialist_count={}, exact_count={:?}, query={:?}, request={}",
                        idx,
                        expected_count,
                        specialist_count,
                        exact_count,
                        query,
                        String::from_utf8_lossy(&request)
                    );
                }
            }
        }

        if let Some(exact_count) = exact_count {
            if exact_count != specialist_count {
                mismatches += 1;
                if mismatches <= 10 {
                    eprintln!(
                        "EXACT MISMATCH idx={}, exact_count={}, specialist_count={}, request={}",
                        idx,
                        exact_count,
                        specialist_count,
                        String::from_utf8_lossy(&request)
                    );
                }
            }
        }

        total += 1;
    }

    eprintln!("\nVerification complete:");
    eprintln!("  Total queries: {}", total);
    eprintln!("  Mismatches: {}", mismatches);

    if diag {
        let metadata = index.metadata();
        eprintln!("\nIndex diagnostics:");
        eprintln!(
            "  reference_count={} partition_count={} node_count={} block_count={}",
            metadata.reference_count,
            metadata.partition_count,
            metadata.node_count,
            metadata.block_count
        );
        print_metric("partitions_visited", &mut partitions_visited);
        print_metric("nodes_visited", &mut nodes_visited);
        print_metric("leaves_scanned", &mut leaves_scanned);
        print_metric("blocks_scanned", &mut blocks_scanned);
    }

    if mismatches > 0 {
        std::process::exit(1);
    }
}

fn print_metric(name: &str, values: &mut [u32]) {
    if values.is_empty() {
        eprintln!("  {}: avg=0.00 p95=0 p99=0", name);
        return;
    }

    values.sort_unstable();
    let sum: u64 = values.iter().map(|&v| v as u64).sum();
    let avg = sum as f64 / values.len() as f64;
    let p95 = percentile(values, 0.95);
    let p99 = percentile(values, 0.99);
    eprintln!("  {}: avg={:.2} p95={} p99={}", name, avg, p95, p99);
}

fn percentile(values: &[u32], percentile: f64) -> u32 {
    let idx = ((values.len() - 1) as f64 * percentile).ceil() as usize;
    values[idx]
}
