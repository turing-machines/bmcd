use std::collections::hash_map::DefaultHasher;
use std::hash::Hash;
use std::hash::Hasher;
pub mod app_persistency;
pub mod binary_persistency;
pub mod error;

#[inline]
fn default_hash<T: Hash>(item: T) -> u64 {
    let mut hasher = DefaultHasher::new();
    item.hash(&mut hasher);
    hasher.finish()
}
