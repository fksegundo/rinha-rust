pub mod api;
pub mod fd_passing;
pub mod http;
pub mod index;
pub mod vector;

pub const DIMS: usize = 14;
pub const PACKED_DIMS: usize = 16;
pub const SCALE: i16 = 10000;
pub const K: usize = 5;

pub type QueryVector = [i16; PACKED_DIMS];
