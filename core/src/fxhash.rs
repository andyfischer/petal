//! A small, fast, non-cryptographic hasher for the VM's own tables.
//!
//! Record fields are looked up by name on every `r.field`, and the default
//! SipHash spends most of such a lookup hashing a five-byte key. This is the
//! rotate-xor-multiply hash rustc uses internally (FxHash), taking eight bytes
//! per step. It is not HashDoS-resistant, which is fine for maps whose keys
//! are a program's field names.

use std::hash::{BuildHasherDefault, Hasher};

const SEED: u64 = 0x51_7c_c1_b7_27_22_0a_95;

#[derive(Default, Clone, Copy)]
pub struct FxHasher {
    hash: u64,
}

impl FxHasher {
    #[inline(always)]
    fn add(&mut self, word: u64) {
        self.hash = (self.hash.rotate_left(5) ^ word).wrapping_mul(SEED);
    }
}

impl Hasher for FxHasher {
    #[inline]
    fn write(&mut self, bytes: &[u8]) {
        let mut chunks = bytes.chunks_exact(8);
        for c in &mut chunks {
            self.add(u64::from_le_bytes(c.try_into().unwrap()));
        }
        let rest = chunks.remainder();
        if !rest.is_empty() {
            let mut buf = [0u8; 8];
            buf[..rest.len()].copy_from_slice(rest);
            self.add(u64::from_le_bytes(buf));
        }
    }

    #[inline]
    fn write_u8(&mut self, i: u8) {
        self.add(i as u64);
    }

    #[inline]
    fn write_u32(&mut self, i: u32) {
        self.add(i as u64);
    }

    #[inline]
    fn write_u64(&mut self, i: u64) {
        self.add(i);
    }

    #[inline]
    fn write_usize(&mut self, i: usize) {
        self.add(i as u64);
    }

    #[inline]
    fn finish(&self) -> u64 {
        self.hash
    }
}

/// `BuildHasher` for [`FxHasher`]; the `S` of an `IndexMap`/`HashMap`.
pub type FxBuildHasher = BuildHasherDefault<FxHasher>;

#[cfg(test)]
mod tests {
    use super::*;
    use std::hash::{BuildHasher, Hash};

    fn h(s: &str) -> u64 {
        let mut hasher = FxBuildHasher::default().build_hasher();
        s.hash(&mut hasher);
        hasher.finish()
    }

    #[test]
    fn equal_strings_hash_equal_and_different_ones_differ() {
        assert_eq!(h("pos"), h("pos"));
        assert_ne!(h("pos"), h("rot"));
        assert_ne!(h("abcdefgh"), h("abcdefgi"));
        // `str`'s Hash appends a terminator, so a prefix is not a collision.
        assert_ne!(h("ab"), h("abc"));
    }
}
