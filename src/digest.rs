//! A small, stable content hash.
//!
//! FNV-1a: dependency free, and — unlike the standard hasher — guaranteed
//! not to change between releases. Used to notice that a Task's definition
//! changed and to give each Task a fixed place in its jitter window, neither
//! of which may shift under a rebuild. Not a security primitive.

const OFFSET_BASIS: u64 = 0xcbf2_9ce4_8422_2325;
const PRIME: u64 = 0x0000_0100_0000_01b3;

pub fn fnv1a(bytes: &[u8]) -> u64 {
    bytes.iter().fold(OFFSET_BASIS, |hash, byte| {
        (hash ^ u64::from(*byte)).wrapping_mul(PRIME)
    })
}

/// A short printable digest of some content.
pub fn of(bytes: &[u8]) -> String {
    format!("{:016x}", fnv1a(bytes))
}
