//! XXH3-64 (default secret, seed zero) fingerprint hashing.

use memoria_application::ports::{FingerprintHasher, HashStream};
use memoria_domain::Hash64;
use xxhash_rust::xxh3::{Xxh3, xxh3_64 as raw_xxh3_64, xxh3_128 as raw_xxh3_128};

/// XXH3-64 with the default secret and seed zero.
pub fn xxh3_64(bytes: &[u8]) -> u64 {
    raw_xxh3_64(bytes)
}

/// XXH3-128 with the default secret and seed zero, in big-endian order.
/// The `memoria.lock` frame stores these sixteen bytes as its trailer.
pub fn xxh3_128(bytes: &[u8]) -> [u8; 16] {
    raw_xxh3_128(bytes).to_be_bytes()
}

#[derive(Debug, Default, Clone, Copy)]
pub struct Xxh3Hasher;

struct Stream(Xxh3);

impl HashStream for Stream {
    fn update(&mut self, bytes: &[u8]) {
        self.0.update(bytes);
    }

    fn finish(self: Box<Self>) -> Hash64 {
        Hash64(self.0.digest())
    }
}

impl FingerprintHasher for Xxh3Hasher {
    fn hash(&self, bytes: &[u8]) -> Hash64 {
        Hash64(raw_xxh3_64(bytes))
    }

    fn stream(&self) -> Box<dyn HashStream> {
        Box::new(Stream(Xxh3::new()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_checksum_is_big_endian_xxh3_128() {
        // Reference XXH3_128bits values from xxHash C 0.8.3.
        assert_eq!(
            super::xxh3_128(b""),
            [
                0x99, 0xaa, 0x06, 0xd3, 0x01, 0x47, 0x98, 0xd8, 0x60, 0x01, 0xc3, 0x24, 0x46, 0x8d,
                0x49, 0x7f
            ]
        );
        assert_eq!(super::xxh3_128(b"a").len(), 16);
        assert_ne!(super::xxh3_128(b"a"), super::xxh3_128(b"b"));
    }

    #[test]
    fn identifies_xxh3_64_for_one_shot_and_streaming() {
        let hasher = Xxh3Hasher;
        assert_eq!(memoria_domain::canonical::HASH_ALGORITHM, "xxh3-64-seed0");
        // Legacy XXH64 gives ef46db3751d8e999 for empty input.
        assert_eq!(hasher.hash(b"").to_hex(), "2d06800538d394c2");
        assert_eq!(hasher.hash(b"a").to_hex(), "e6c632b61e964e1f");
        assert_eq!(hasher.hash(b"abc").to_hex(), "78af5f94892f3950");
        let mut stream = hasher.stream();
        stream.update(b"ab");
        stream.update(b"c");
        assert_eq!(stream.finish().to_hex(), "78af5f94892f3950");
    }

    #[test]
    fn reference_vectors_cover_length_branches_and_stream_boundaries() {
        // Independent xxHash C 0.8.3 XXH3_64bits results for this byte pattern.
        let bytes: Vec<u8> = (0..4097).map(|i| (i * 131 + 17) as u8).collect();
        let vectors = [
            (0, 0x2d06800538d394c2),
            (1, 0xf319fe2bdfcdfebd),
            (3, 0xa107bb65b715c89b),
            (4, 0x509f0567aa8a3123),
            (8, 0xb1433dc39b7f946e),
            (9, 0xfe2542440b36ddc7),
            (16, 0x715189ff3dcdcff6),
            (17, 0x7d33163b8af0179c),
            (64, 0x0991d97cd58dd82d),
            (128, 0xee847f7fcef4ddbc),
            (129, 0x7e3e7b750239d4fc),
            (160, 0x0f2adb08915ea24e),
            (240, 0x44089a144aade02d),
            (241, 0xed93572e52acac83),
            (256, 0x5356e48742805b6a),
            (257, 0x90215816e3d19240),
            (1023, 0x90e9c9b2131acc97),
            (1024, 0xf0c5763fadfacd25),
            (1025, 0xd9b8e93ef3fbe416),
            (4097, 0x605c55fffc03cdaf),
        ];
        for (len, expected) in vectors {
            assert_eq!(
                Xxh3Hasher.hash(&bytes[..len]),
                Hash64(expected),
                "len={len}"
            );
            for chunk in [1, 3, 8, 63, 64, 65, 127, 128, 240, 255, 256, 257, 1024] {
                let mut stream = Xxh3Hasher.stream();
                stream.update(&[]);
                for part in bytes[..len].chunks(chunk) {
                    stream.update(part);
                    stream.update(&[]);
                }
                assert_eq!(
                    stream.finish(),
                    Hash64(expected),
                    "len={len}, chunk={chunk}"
                );
            }
        }
    }

    #[test]
    fn streaming_is_invariant_under_every_two_part_split() {
        let bytes: Vec<u8> = (0..1025).map(|i| (i * 131 + 17) as u8).collect();
        for len in [0, 1, 16, 17, 128, 129, 240, 241, 256, 257, 1024, 1025] {
            let expected = Xxh3Hasher.hash(&bytes[..len]);
            for split in 0..=len {
                let mut stream = Xxh3Hasher.stream();
                stream.update(&bytes[..split]);
                stream.update(&bytes[split..len]);
                assert_eq!(stream.finish(), expected, "len={len}, split={split}");
            }
        }
    }
}
