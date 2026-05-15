use crate::index::compute_partition_key;
use crate::index::format::IndexWriter;
use crate::{DIMS, PACKED_DIMS, QueryVector, SCALE};
use flate2::read::GzDecoder;
use std::collections::HashMap;
use std::io::Read;

pub const LANES: usize = 8;

#[derive(Clone)]
pub struct Reference {
    pub vector: QueryVector,
    pub label: u8,
}

pub fn load_references(path: &str) -> Result<Vec<Reference>, String> {
    let file = std::fs::File::open(path).map_err(|e| e.to_string())?;
    let mut decoder = GzDecoder::new(file);
    let mut json_str = String::new();
    decoder
        .read_to_string(&mut json_str)
        .map_err(|e| e.to_string())?;

    let json: serde_json::Value =
        serde_json::from_str(&json_str).map_err(|e| format!("JSON parse error: {}", e))?;

    let array = json.as_array().ok_or("expected top-level array")?;

    let mut references = Vec::with_capacity(array.len());

    for item in array {
        let vec = item
            .get("vector")
            .and_then(|v| v.as_array())
            .ok_or("missing vector array")?;
        if vec.len() != DIMS {
            return Err(format!("expected {} dims, got {}", DIMS, vec.len()));
        }

        let mut vector = [0i16; PACKED_DIMS];
        for (i, val) in vec.iter().enumerate() {
            let f = val.as_f64().ok_or("non-numeric vector value")?;
            vector[i] = quantize(f);
        }

        let label_str = item
            .get("label")
            .and_then(|v| v.as_str())
            .ok_or("missing label")?;
        let label = if label_str == "fraud" { 1u8 } else { 0u8 };

        references.push(Reference { vector, label });
    }

    Ok(references)
}

#[inline]
fn quantize(value: f64) -> i16 {
    if value <= -1.0 {
        -SCALE
    } else if value <= 0.0 {
        0
    } else if value >= 1.0 {
        SCALE
    } else {
        (value * SCALE as f64).round() as i16
    }
}

struct NodeEntry {
    left: i32,
    right: i32,
    start: usize,
    len: usize,
    min: QueryVector,
    max: QueryVector,
}

pub fn build_index(
    references: Vec<Reference>,
    leaf_size: usize,
    _flat_threshold: usize,
) -> Result<Vec<u8>, String> {
    let leaf_size = leaf_size.clamp(32, 2048);
    let mut writer = IndexWriter::new();
    writer.write_header(references.len() as i32)?;

    // Partition references
    let mut partitions: HashMap<u32, Vec<usize>> = HashMap::new();
    for (idx, ref_item) in references.iter().enumerate() {
        let key = compute_partition_key(&ref_item.vector);
        partitions.entry(key).or_default().push(idx);
    }

    let mut all_blocks: Vec<(QueryVector, u8)> = Vec::new();
    let mut nodes: Vec<NodeEntry> = Vec::new();
    let mut partition_meta: Vec<(u32, usize)> = Vec::new();

    let mut sorted_keys: Vec<u32> = partitions.keys().copied().collect();
    sorted_keys.sort_unstable();

    for key in &sorted_keys {
        let indices = &partitions[key];
        let root = build_node(&references, indices, leaf_size, &mut all_blocks, &mut nodes);
        partition_meta.push((*key, root));
    }

    let partition_count = partition_meta.len() as i32;
    writer.write_partition_count(partition_count)?;
    let node_count = nodes.len() as i32;
    writer.write_node_count(node_count)?;

    // Write partition directory
    for (key, root) in &partition_meta {
        let root_node = &nodes[*root];
        writer.write_partition_entry(*key, *root, root_node.len, root_node.min, root_node.max)?;
    }

    // Write node directory
    for node in &nodes {
        let block_start = node.start / LANES;
        writer.write_node_entry(
            node.left,
            node.right,
            block_start,
            node.len,
            node.min,
            node.max,
        )?;
    }

    // Write vector blocks in AoSoA layout (all vectors first, then all labels)
    let total_blocks = all_blocks.len() / LANES;
    writer.write_block_count(total_blocks as i32)?;

    for b in 0..total_blocks {
        for d in 0..DIMS {
            for l in 0..LANES {
                let (vec, _) = all_blocks[b * LANES + l];
                writer.write_i16(vec[d])?;
            }
        }
    }

    for b in 0..total_blocks {
        for l in 0..LANES {
            let (_, label) = all_blocks[b * LANES + l];
            writer.write_u8(label)?;
        }
    }

    Ok(writer.into_bytes())
}

fn build_node(
    references: &[Reference],
    indices: &[usize],
    leaf_size: usize,
    all_blocks: &mut Vec<(QueryVector, u8)>,
    nodes: &mut Vec<NodeEntry>,
) -> usize {
    let mut min = [i16::MAX; PACKED_DIMS];
    let mut max = [i16::MIN; PACKED_DIMS];
    for &idx in indices {
        let ref_item = &references[idx];
        for d in 0..PACKED_DIMS {
            min[d] = min[d].min(ref_item.vector[d]);
            max[d] = max[d].max(ref_item.vector[d]);
        }
    }

    let node_idx = nodes.len();
    nodes.push(NodeEntry {
        left: -1,
        right: -1,
        start: 0,
        len: 0,
        min,
        max,
    });

    if indices.len() <= leaf_size {
        let leaf_start = all_blocks.len();
        let blocks = (indices.len() + LANES - 1) / LANES;

        for b in 0..blocks {
            for l in 0..LANES {
                let i = b * LANES + l;
                if i < indices.len() {
                    let ref_item = &references[indices[i]];
                    all_blocks.push((ref_item.vector, ref_item.label));
                } else {
                    all_blocks.push(([0i16; PACKED_DIMS], 0u8));
                }
            }
        }

        nodes[node_idx] = NodeEntry {
            left: -1,
            right: -1,
            start: leaf_start,
            len: indices.len(),
            min,
            max,
        };
        return node_idx;
    }

    let split_dim = widest_dimension(&min, &max);
    let mut sorted = indices.to_vec();
    sorted.sort_unstable_by(|&a, &b| {
        references[a].vector[split_dim].cmp(&references[b].vector[split_dim])
    });

    let left_len = sorted.len() / 2;
    let (left_indices, right_indices) = sorted.split_at(left_len);

    let left_node = build_node(references, left_indices, leaf_size, all_blocks, nodes);
    let right_node = build_node(references, right_indices, leaf_size, all_blocks, nodes);

    let left_info = &nodes[left_node];
    let right_info = &nodes[right_node];

    nodes[node_idx] = NodeEntry {
        left: left_node as i32,
        right: right_node as i32,
        start: left_info.start,
        len: left_info.len + right_info.len,
        min,
        max,
    };

    node_idx
}

fn widest_dimension(min: &QueryVector, max: &QueryVector) -> usize {
    let mut best_dim = 0usize;
    let mut best_width = i16::MIN;
    for d in 0..DIMS {
        let width = max[d] - min[d];
        if width > best_width {
            best_width = width;
            best_dim = d;
        }
    }
    best_dim
}
