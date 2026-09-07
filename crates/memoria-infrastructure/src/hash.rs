//! xxHash64 (seed zero) fingerprint hashing.

use memoria_application::ports::{FingerprintHasher, HashStream};
use memoria_domain::Hash64;
use xxhash_rust::xxh64::{Xxh64, xxh64};

#[derive(Debug, Default, Clone, Copy)]
pub struct Xxh64Hasher;

struct Stream(Xxh64);

impl HashStream for Stream {
    fn update(&mut self, bytes: &[u8]) {
        self.0.update(bytes);
    }

    fn finish(self: Box<Self>) -> Hash64 {
        Hash64(self.0.digest())
    }
}

impl FingerprintHasher for Xxh64Hasher {
    fn hash(&self, bytes: &[u8]) -> Hash64 {
        Hash64(xxh64(bytes, 0))
    }

    fn stream(&self) -> Box<dyn HashStream> {
        Box::new(Stream(Xxh64::new(0)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_published_vectors() {
        let hasher = Xxh64Hasher;
        assert_eq!(hasher.hash(b"").to_hex(), "ef46db3751d8e999");
        assert_eq!(hasher.hash(b"a").to_hex(), "d24ec4f1a98c6e5b");
        assert_eq!(hasher.hash(b"abc").to_hex(), "44bc2cf5ad770999");
        let mut stream = hasher.stream();
        stream.update(b"ab");
        stream.update(b"c");
        assert_eq!(stream.finish().to_hex(), "44bc2cf5ad770999");
    }
}
