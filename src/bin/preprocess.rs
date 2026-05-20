use std::env;

fn main() {
    let args: Vec<String> = env::args().collect();
    if args.len() < 3 {
        eprintln!("Usage: {} <references.json.gz> <output.idx>", args[0]);
        std::process::exit(1);
    }

    let input_path = &args[1];
    let output_path = &args[2];

    eprintln!("Loading references from {}...", input_path);
    let references = rinha_rust::index::build::load_references(input_path)
        .unwrap_or_else(|e| panic!("failed to load references: {}", e));
    eprintln!("Loaded {} references", references.len());

    let leaf_size: usize = env::var("RINHA_LEAF_SIZE")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(256);
    let flat_threshold: usize = env::var("RINHA_FLAT_THRESHOLD")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(128);

    eprintln!(
        "Building index with leaf_size={}, flat_threshold={}...",
        leaf_size, flat_threshold
    );
    let index_bytes = rinha_rust::index::build::build_index(references, leaf_size, flat_threshold)
        .unwrap_or_else(|e| panic!("failed to build index: {}", e));

    let len = index_bytes.len();
    std::fs::write(output_path, index_bytes)
        .unwrap_or_else(|e| panic!("failed to write index: {}", e));

    eprintln!("Index written to {} ({} bytes)", output_path, len);
}
