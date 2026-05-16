pub mod build;
pub mod format;

#[cfg(test)]
mod tests;

use crate::{DIMS, K, PACKED_DIMS, QueryVector, SCALE};
use std::fs::File;
use std::mem;
use std::os::fd::AsRawFd;
use std::ptr;
use std::slice;

const MAGIC: &[u8; 8] = b"RNSPCST1";
const LANES: usize = 8;
const MAX_PARTITIONS: usize = 512;
const TREE_STACK_CAPACITY: usize = 128;

pub struct SpecialistIndex {
    _mapping: MmapRegion,
    reference_count: usize,
    partitions: Vec<Partition>,
    nodes: Vec<Node>,
    vectors: *const i16,
    vectors_len: usize,
    labels: *const u8,
    labels_len: usize,
    has_avx2: bool,
    search_mode: SearchMode,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SearchMode {
    Exact,
    Specialist,
    KeyFirst,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct IndexMetadata {
    pub reference_count: usize,
    pub partition_count: usize,
    pub node_count: usize,
    pub block_count: usize,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct SearchStats {
    pub partitions_visited: u32,
    pub nodes_visited: u32,
    pub leaves_scanned: u32,
    pub blocks_scanned: u32,
}

struct MmapRegion {
    ptr: *mut u8,
    len: usize,
}

unsafe impl Send for MmapRegion {}
unsafe impl Sync for MmapRegion {}
unsafe impl Send for SpecialistIndex {}
unsafe impl Sync for SpecialistIndex {}

impl MmapRegion {
    pub fn open(path: &str) -> Result<Self, String> {
        let file = File::open(path).map_err(|e| e.to_string())?;
        let len = file.metadata().map_err(|e| e.to_string())?.len() as usize;
        if len == 0 {
            return Err("empty file".to_string());
        }
        unsafe {
            let ptr = libc::mmap(
                ptr::null_mut(),
                len,
                libc::PROT_READ,
                libc::MAP_PRIVATE,
                file.as_raw_fd(),
                0,
            );
            if ptr == libc::MAP_FAILED {
                return Err(std::io::Error::last_os_error().to_string());
            }
            Ok(Self {
                ptr: ptr.cast::<u8>(),
                len,
            })
        }
    }

    fn as_slice(&self) -> &[u8] {
        unsafe { slice::from_raw_parts(self.ptr.cast_const(), self.len) }
    }
}

impl Drop for MmapRegion {
    fn drop(&mut self) {
        unsafe {
            libc::munmap(self.ptr.cast::<libc::c_void>(), self.len);
        }
    }
}

#[derive(Clone)]
struct Partition {
    key: u32,
    root: usize,
    min: [i16; PACKED_DIMS],
    max: [i16; PACKED_DIMS],
}

#[derive(Clone)]
struct Node {
    left: i32,
    right: i32,
    start: usize,
    len: usize,
    min: [i16; PACKED_DIMS],
    max: [i16; PACKED_DIMS],
}

impl SpecialistIndex {
    pub fn open(path: &str) -> Result<Self, String> {
        let mapping = MmapRegion::open(path)?;
        let bytes = mapping.as_slice();
        if bytes.len() < 8 {
            return Err("file too short".to_string());
        }
        let magic: &[u8; 8] = bytes[..8].try_into().unwrap();
        if magic != MAGIC {
            return Err(format!("unknown magic: {:?}", magic));
        }
        Self::load(mapping)
    }

    fn load(mapping: MmapRegion) -> Result<Self, String> {
        let bytes = mapping.as_slice();
        let mut cursor = 8usize;

        let scale = read_i32(bytes, &mut cursor)?;
        let packed_dims = read_i32(bytes, &mut cursor)? as usize;
        let reference_count = read_i32(bytes, &mut cursor)? as usize;
        let partition_count = read_i32(bytes, &mut cursor)? as usize;
        let node_count = read_i32(bytes, &mut cursor)? as usize;
        let total_blocks = read_i32(bytes, &mut cursor)? as usize;

        if scale != SCALE as i32 {
            return Err(format!(
                "invalid index scale: expected {}, got {}",
                SCALE, scale
            ));
        }

        if packed_dims != PACKED_DIMS {
            return Err("invalid packed dimensions".to_string());
        }

        let mut partitions = Vec::with_capacity(partition_count);
        for _ in 0..partition_count {
            let key = read_i32(bytes, &mut cursor)? as u32;
            let root = read_i32(bytes, &mut cursor)? as usize;
            let _start = read_i32(bytes, &mut cursor)?;
            let _len = read_i32(bytes, &mut cursor)?;
            let min = read_i16_array(bytes, &mut cursor)?;
            let max = read_i16_array(bytes, &mut cursor)?;
            partitions.push(Partition {
                key,
                root,
                min,
                max,
            });
        }

        let mut nodes = Vec::with_capacity(node_count);
        for _ in 0..node_count {
            let left = read_i32(bytes, &mut cursor)?;
            let right = read_i32(bytes, &mut cursor)?;
            let start = read_i32(bytes, &mut cursor)? as usize;
            let len = read_i32(bytes, &mut cursor)? as usize;
            let min = read_i16_array(bytes, &mut cursor)?;
            let max = read_i16_array(bytes, &mut cursor)?;
            nodes.push(Node {
                left,
                right,
                start,
                len,
                min,
                max,
            });
        }

        let vectors_len = total_blocks * DIMS * LANES;
        let vectors_bytes = vectors_len * mem::size_of::<i16>();
        if cursor % mem::align_of::<i16>() != 0 {
            return Err("unaligned vectors section".to_string());
        }
        if cursor + vectors_bytes > bytes.len() {
            return Err("truncated vectors".to_string());
        }
        let vectors = unsafe { bytes.as_ptr().add(cursor).cast::<i16>() };
        cursor += vectors_bytes;

        let labels_len = total_blocks * LANES;
        if cursor + labels_len > bytes.len() {
            return Err("truncated labels".to_string());
        }
        let labels = unsafe { bytes.as_ptr().add(cursor) };

        let has_avx2 = cfg!(target_arch = "x86_64") && std::arch::is_x86_feature_detected!("avx2");
        let search_mode = match std::env::var("RINHA_SEARCH_MODE").as_deref() {
            Ok("exact") => SearchMode::Exact,
            Ok("key-first") | Ok("key_first") => SearchMode::KeyFirst,
            _ => SearchMode::Specialist,
        };

        eprintln!(
            "[RNSPCST1] loaded: {} partitions, {} nodes, {} blocks, avx2={}, mode={:?}",
            partition_count, node_count, total_blocks, has_avx2, search_mode
        );

        let index = Self {
            _mapping: mapping,
            reference_count,
            partitions,
            nodes,
            vectors,
            vectors_len,
            labels,
            labels_len,
            has_avx2,
            search_mode,
        };
        index.advise_hugepages();
        Ok(index)
    }

    pub fn predict_fraud_count(&self, query: &QueryVector) -> u8 {
        self.predict_fraud_count_inner(query, None)
    }

    pub fn predict_fraud_count_with_stats(&self, query: &QueryVector) -> (u8, SearchStats) {
        let mut stats = SearchStats::default();
        let count = self.predict_fraud_count_inner(query, Some(&mut stats));
        (count, stats)
    }

    pub fn metadata(&self) -> IndexMetadata {
        IndexMetadata {
            reference_count: self.reference_count,
            partition_count: self.partitions.len(),
            node_count: self.nodes.len(),
            block_count: self.vectors_len / (DIMS * LANES),
        }
    }

    fn predict_fraud_count_inner(
        &self,
        query: &QueryVector,
        mut stats: Option<&mut SearchStats>,
    ) -> u8 {
        let mut best_dists = [i64::MAX; K];
        let mut best_labels = [0u8; K];

        if self.search_mode == SearchMode::Exact {
            // Flat scan over all vectors (for verification)
            let vectors = self.vectors();
            let labels = self.labels();
            let total_blocks = self.vectors_len / (DIMS * LANES);
            for b in 0..total_blocks {
                let block_base = b * DIMS * LANES;
                let dists = if self.has_avx2 {
                    scan_block_avx2(vectors, block_base, query)
                } else {
                    scan_block_scalar(vectors, block_base, query)
                };
                let labels_base = b * LANES;
                let remaining = self.reference_count.saturating_sub(b * LANES);
                let lane_count = remaining.min(LANES);
                for i in 0..lane_count {
                    insert_best(
                        dists[i],
                        labels[labels_base + i],
                        &mut best_dists,
                        &mut best_labels,
                    );
                }
            }
        } else if self.search_mode == SearchMode::Specialist {
            let mut partition_entries = [(0i64, 0usize); MAX_PARTITIONS];
            let mut partition_len = 0usize;

            for (idx, partition) in self.partitions.iter().enumerate() {
                let bound = lower_bound_box(query, &partition.min, &partition.max, self.has_avx2);
                partition_entries[partition_len] = (bound, idx);
                partition_len += 1;
            }

            partition_entries[..partition_len].sort_unstable_by_key(|&(bound, _)| bound);

            for i in 0..partition_len {
                let (bound, idx) = partition_entries[i];
                if bound >= best_dists[K - 1] {
                    break;
                }
                self.search_node_iterative(
                    self.partitions[idx].root,
                    bound,
                    query,
                    &mut best_dists,
                    &mut best_labels,
                    stats.as_deref_mut(),
                );
            }
        } else {
            let query_key = compute_partition_key(query);
            let mut partition_entries = [(0i64, 0usize); MAX_PARTITIONS];
            let mut partition_len = 0usize;

            for (idx, partition) in self.partitions.iter().enumerate() {
                let bound = lower_bound_box(query, &partition.min, &partition.max, self.has_avx2);
                if partition.key == query_key {
                    if bound < best_dists[K - 1] {
                        self.search_node_iterative(
                            partition.root,
                            bound,
                            query,
                            &mut best_dists,
                            &mut best_labels,
                            stats.as_deref_mut(),
                        );
                    }
                } else {
                    partition_entries[partition_len] = (bound, idx);
                    partition_len += 1;
                }
            }

            partition_entries[..partition_len].sort_unstable_by_key(|&(bound, _)| bound);

            for i in 0..partition_len {
                let (bound, idx) = partition_entries[i];
                if bound >= best_dists[K - 1] {
                    break;
                }
                self.search_node_iterative(
                    self.partitions[idx].root,
                    bound,
                    query,
                    &mut best_dists,
                    &mut best_labels,
                    stats.as_deref_mut(),
                );
            }
        }

        best_labels.iter().map(|&l| l as u32).sum::<u32>() as u8
    }

    pub fn predict_fraud_count_exact(&self, query: &QueryVector) -> u8 {
        let mut best_dists = [i64::MAX; K];
        let mut best_labels = [0u8; K];
        let vectors = self.vectors();
        let labels = self.labels();
        let total_blocks = self.vectors_len / (DIMS * LANES);
        for b in 0..total_blocks {
            let block_base = b * DIMS * LANES;
            let dists = if self.has_avx2 {
                scan_block_avx2(vectors, block_base, query)
            } else {
                scan_block_scalar(vectors, block_base, query)
            };
            let labels_base = b * LANES;
            let remaining = self.reference_count.saturating_sub(b * LANES);
            let lane_count = remaining.min(LANES);
            for i in 0..lane_count {
                insert_best(
                    dists[i],
                    labels[labels_base + i],
                    &mut best_dists,
                    &mut best_labels,
                );
            }
        }
        best_labels.iter().map(|&l| l as u32).sum::<u32>() as u8
    }

    fn search_node_iterative(
        &self,
        root: usize,
        root_bound: i64,
        query: &QueryVector,
        best_dists: &mut [i64; K],
        best_labels: &mut [u8; K],
        mut stats: Option<&mut SearchStats>,
    ) {
        if let Some(stats) = stats.as_deref_mut() {
            stats.partitions_visited += 1;
        }

        let mut stack_nodes = [0usize; TREE_STACK_CAPACITY];
        let mut stack_bounds = [0i64; TREE_STACK_CAPACITY];
        let mut stack_len = 0usize;

        let mut current = root;
        let mut current_bound = root_bound;

        loop {
            if current_bound <= best_dists[K - 1] {
                let node = &self.nodes[current];
                if let Some(stats) = stats.as_deref_mut() {
                    stats.nodes_visited += 1;
                }
                if node.left < 0 || node.right < 0 {
                    self.scan_leaf(node, query, best_dists, best_labels, stats.as_deref_mut());
                } else {
                    let l = node.left as usize;
                    let r = node.right as usize;

                    #[cfg(target_arch = "x86_64")]
                    unsafe {
                        use std::arch::x86_64::*;
                        _mm_prefetch((&self.nodes[r]) as *const _ as *const i8, _MM_HINT_T0);
                    }

                    let lb = lower_bound_box(
                        query,
                        &self.nodes[l].min,
                        &self.nodes[l].max,
                        self.has_avx2,
                    );
                    let rb = lower_bound_box(
                        query,
                        &self.nodes[r].min,
                        &self.nodes[r].max,
                        self.has_avx2,
                    );

                    let (near_idx, near_bound, far_idx, far_bound) = if lb <= rb {
                        (l, lb, r, rb)
                    } else {
                        (r, rb, l, lb)
                    };

                    if far_bound <= best_dists[K - 1] && stack_len < TREE_STACK_CAPACITY {
                        stack_nodes[stack_len] = far_idx;
                        stack_bounds[stack_len] = far_bound;
                        stack_len += 1;
                    }

                    if near_bound <= best_dists[K - 1] {
                        current = near_idx;
                        current_bound = near_bound;
                        continue;
                    }
                }
            }

            if stack_len == 0 {
                break;
            }

            stack_len -= 1;
            current = stack_nodes[stack_len];
            current_bound = stack_bounds[stack_len];
        }
    }

    fn scan_leaf(
        &self,
        node: &Node,
        query: &QueryVector,
        best_dists: &mut [i64; K],
        best_labels: &mut [u8; K],
        stats: Option<&mut SearchStats>,
    ) {
        let start_block = node.start;
        let blocks = (node.len + LANES - 1) / LANES;
        if let Some(stats) = stats {
            stats.leaves_scanned += 1;
            stats.blocks_scanned += blocks as u32;
        }
        let vectors = self.vectors();
        let labels = self.labels();
        debug_assert!(
            start_block + blocks <= self.vectors_len / (DIMS * LANES),
            "scan_leaf OOB: start_block={}, blocks={}, total_blocks={}",
            start_block,
            blocks,
            self.vectors_len / (DIMS * LANES)
        );

        for b in 0..blocks {
            let block_idx = start_block + b;
            let block_base = block_idx * DIMS * LANES;

            #[cfg(target_arch = "x86_64")]
            if b + 1 < blocks {
                unsafe {
                    use std::arch::x86_64::*;
                    let next_base = (start_block + b + 1) * DIMS * LANES;
                    let ptr = self.vectors.add(next_base) as *const i8;
                    _mm_prefetch(ptr, _MM_HINT_T0);
                    _mm_prefetch(ptr.add(64), _MM_HINT_T0);
                    _mm_prefetch(ptr.add(128), _MM_HINT_T0);
                    _mm_prefetch(ptr.add(192), _MM_HINT_T0);

                    let labels_ptr = self.labels.add((start_block + b + 1) * LANES) as *const i8;
                    _mm_prefetch(labels_ptr, _MM_HINT_T0);
                }
            }

            let dists = if self.has_avx2 {
                scan_block_avx2(vectors, block_base, query)
            } else {
                scan_block_scalar(vectors, block_base, query)
            };
            let labels_base = block_idx * LANES;
            let lane_count = (node.len - b * LANES).min(LANES);
            for i in 0..lane_count {
                insert_best(dists[i], labels[labels_base + i], best_dists, best_labels);
            }
        }
    }

    fn vectors(&self) -> &[i16] {
        unsafe { slice::from_raw_parts(self.vectors, self.vectors_len) }
    }

    fn labels(&self) -> &[u8] {
        unsafe { slice::from_raw_parts(self.labels, self.labels_len) }
    }

    fn advise_hugepages(&self) {
        #[cfg(target_os = "linux")]
        unsafe {
            let vptr = self.vectors as *mut libc::c_void;
            let vlen = self.vectors_len * mem::size_of::<i16>();
            libc::madvise(vptr, vlen, libc::MADV_HUGEPAGE);

            let lptr = self.labels as *mut libc::c_void;
            let llen = self.labels_len;
            libc::madvise(lptr, llen, libc::MADV_HUGEPAGE);
        }
    }
}

// ─── Helpers ─────────────────────────────────────────────────────────────────

pub fn compute_partition_key(vector: &QueryVector) -> u32 {
    let mut key = 0u32;
    if vector[5] >= 0 {
        key |= 1 << 0; // has_last_tx
    }
    if vector[9] > 0 {
        key |= 1 << 1; // is_online
    }
    if vector[10] > 0 {
        key |= 1 << 2; // card_present
    }
    if vector[11] > 0 {
        key |= 1 << 3; // unknown_merchant
    }

    let mcc_bucket = match vector[12] {
        ..=2047 => 0,
        2048..=4095 => 1,
        4096..=6143 => 2,
        _ => 3,
    };
    key |= mcc_bucket << 4;

    if vector[2] > 4096 {
        key |= 1 << 6; // amount > 5x customer avg
    }
    if vector[8] > 2048 {
        key |= 1 << 7; // tx_count_24h > 5
    }

    key
}

#[inline(always)]
fn insert_best(dist: i64, label: u8, best_dists: &mut [i64; K], best_labels: &mut [u8; K]) {
    if dist >= best_dists[K - 1] {
        return;
    }
    let mut pos = K - 1;
    while pos > 0 && dist < best_dists[pos - 1] {
        best_dists[pos] = best_dists[pos - 1];
        best_labels[pos] = best_labels[pos - 1];
        pos -= 1;
    }
    best_dists[pos] = dist;
    best_labels[pos] = label;
}

#[inline(always)]
fn scan_block_avx2(vectors: &[i16], block_base: usize, query: &QueryVector) -> [i64; LANES] {
    #[cfg(target_arch = "x86_64")]
    unsafe {
        use std::arch::x86_64::*;
        let mut sum64_lo = _mm256_setzero_si256();
        let mut sum64_hi = _mm256_setzero_si256();
        let mut sum32 = _mm256_setzero_si256();

        for d in 0..DIMS {
            let q_vec = _mm_set1_epi16(query[d]);
            let v_ptr = vectors.as_ptr().add(block_base + d * LANES);
            let v_vec = _mm_loadu_si128(v_ptr as *const __m128i);
            let diff = _mm_sub_epi16(q_vec, v_vec);
            let diff32 = _mm256_cvtepi16_epi32(diff);
            let sq = _mm256_mullo_epi32(diff32, diff32);
            sum32 = _mm256_add_epi32(sum32, sq);

            if (d + 1) % 4 == 0 {
                let lo = _mm256_cvtepi32_epi64(_mm256_castsi256_si128(sum32));
                let hi = _mm256_cvtepi32_epi64(_mm256_extracti128_si256(sum32, 1));
                sum64_lo = _mm256_add_epi64(sum64_lo, lo);
                sum64_hi = _mm256_add_epi64(sum64_hi, hi);
                sum32 = _mm256_setzero_si256();
            }
        }

        let lo = _mm256_cvtepi32_epi64(_mm256_castsi256_si128(sum32));
        let hi = _mm256_cvtepi32_epi64(_mm256_extracti128_si256(sum32, 1));
        sum64_lo = _mm256_add_epi64(sum64_lo, lo);
        sum64_hi = _mm256_add_epi64(sum64_hi, hi);

        let mut block_dists = [0i64; LANES];
        _mm256_storeu_si256(block_dists.as_mut_ptr() as *mut __m256i, sum64_lo);
        _mm256_storeu_si256(block_dists.as_mut_ptr().add(4) as *mut __m256i, sum64_hi);

        return block_dists;
    }

    #[cfg(not(target_arch = "x86_64"))]
    scan_block_scalar(vectors, block_base, query)
}

#[inline(always)]
fn scan_block_scalar(vectors: &[i16], block_base: usize, query: &QueryVector) -> [i64; LANES] {
    let mut dists = [0i64; LANES];
    for d in 0..DIMS {
        let q = query[d] as i64;
        let base = block_base + d * LANES;
        for i in 0..LANES {
            let diff = q - vectors[base + i] as i64;
            dists[i] += diff * diff;
        }
    }
    dists
}

#[inline(always)]
fn lower_bound_box(
    query: &QueryVector,
    min: &QueryVector,
    max: &QueryVector,
    has_avx2: bool,
) -> i64 {
    #[cfg(target_arch = "x86_64")]
    if has_avx2 {
        return unsafe { lower_bound_box_avx2(query, min, max) };
    }
    lower_bound_box_scalar(query, min, max)
}

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx2")]
unsafe fn lower_bound_box_avx2(query: &QueryVector, min: &QueryVector, max: &QueryVector) -> i64 {
    use std::arch::x86_64::*;
    unsafe {
        let q = _mm256_loadu_si256(query.as_ptr() as *const __m256i);
        let mn = _mm256_loadu_si256(min.as_ptr() as *const __m256i);
        let mx = _mm256_loadu_si256(max.as_ptr() as *const __m256i);

        let zero = _mm256_setzero_si256();
        let below = _mm256_max_epi16(_mm256_sub_epi16(mn, q), zero);
        let above = _mm256_max_epi16(_mm256_sub_epi16(q, mx), zero);
        let diff = _mm256_max_epi16(below, above);

        let sq = _mm256_madd_epi16(diff, diff);

        let lo = _mm256_cvtepi32_epi64(_mm256_castsi256_si128(sq));
        let hi = _mm256_cvtepi32_epi64(_mm256_extracti128_si256(sq, 1));
        let sum64 = _mm256_add_epi64(lo, hi);

        let sum_hi = _mm256_extracti128_si256(sum64, 1);
        let sum_128 = _mm_add_epi64(_mm256_castsi256_si128(sum64), sum_hi);

        let s0 = _mm_extract_epi64(sum_128, 0);
        let s1 = _mm_extract_epi64(sum_128, 1);

        s0 + s1
    }
}

#[inline(always)]
fn lower_bound_box_scalar(query: &QueryVector, min: &QueryVector, max: &QueryVector) -> i64 {
    let mut sum = 0i64;
    for d in 0..DIMS {
        let q = query[d] as i64;
        let lo = min[d] as i64;
        let hi = max[d] as i64;
        let diff = if q < lo {
            lo - q
        } else if q > hi {
            q - hi
        } else {
            0
        };
        sum += diff * diff;
    }
    sum
}

fn read_i32(bytes: &[u8], cursor: &mut usize) -> Result<i32, String> {
    if *cursor + 4 > bytes.len() {
        return Err("unexpected EOF (i32)".to_string());
    }
    let v = i32::from_le_bytes(bytes[*cursor..*cursor + 4].try_into().unwrap());
    *cursor += 4;
    Ok(v)
}

fn read_i16(bytes: &[u8], cursor: &mut usize) -> Result<i16, String> {
    if *cursor + 2 > bytes.len() {
        return Err("unexpected EOF (i16)".to_string());
    }
    let v = i16::from_le_bytes(bytes[*cursor..*cursor + 2].try_into().unwrap());
    *cursor += 2;
    Ok(v)
}

fn read_i16_array(bytes: &[u8], cursor: &mut usize) -> Result<[i16; PACKED_DIMS], String> {
    let mut arr = [0i16; PACKED_DIMS];
    for x in &mut arr {
        *x = read_i16(bytes, cursor)?;
    }
    Ok(arr)
}
