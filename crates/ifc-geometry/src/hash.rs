//! A fast keyed hash for maps whose keys come from the file: coordinates and record ids.
//!
//! The standard library's SipHash is slow for keys this small. Its keys are also no secret in
//! WebAssembly, which has no randomness of its own, so they are the same on every run. This is
//! NH, the universal hash of UMAC, with a fixed finish: each 64-bit word of the key has its two
//! halves added to a secret and multiplied together, and the products are summed. For two keys
//! of the same length the sums are equal with probability at most 2⁻³², whatever the keys, as
//! long as the secret is random and unknown, so a file cannot choose ids or coordinates that
//! land in one bucket. The host sets the secret from a real random source with [`set_secret`].
//! Code may only look values up through these maps, never walk them, so the secret cannot
//! change the output.

use std::collections::HashMap;
use std::hash::{BuildHasher, Hasher};
use std::sync::atomic::{AtomicU64, Ordering};

/// Words of the secret. Keys longer than this reuse it from the start, which only weakens the
/// bound; every key used here fits.
const WORDS: usize = 4;

/// Digits of π, used until the host calls [`set_secret`], as in native builds and tests.
static SECRET: [AtomicU64; WORDS] = [
    AtomicU64::new(0x243f_6a88_85a3_08d3),
    AtomicU64::new(0x1319_8a2e_0370_7344),
    AtomicU64::new(0xa409_3822_299f_31d0),
    AtomicU64::new(0x082e_fa98_ec4e_6c89),
];

/// Maps built after this call use the new secret. Maps built before keep theirs.
pub fn set_secret(secret: [u64; WORDS]) {
    for (word, value) in SECRET.iter().zip(secret) {
        word.store(value, Ordering::Relaxed);
    }
}

pub type KeyedMap<K, V> = HashMap<K, V, Keyed>;

/// Builds [`KeyedHasher`]s with the secret current when it was made.
#[derive(Clone, Copy, Debug)]
pub struct Keyed {
    secret: [u64; WORDS],
}

impl Default for Keyed {
    fn default() -> Self {
        Self::with_secret(SECRET.each_ref().map(|word| word.load(Ordering::Relaxed)))
    }
}

impl Keyed {
    pub fn with_secret(secret: [u64; WORDS]) -> Self {
        Self { secret }
    }
}

impl BuildHasher for Keyed {
    type Hasher = KeyedHasher;

    fn build_hasher(&self) -> KeyedHasher {
        KeyedHasher { sum: 0, words: 0, secret: self.secret }
    }
}

pub struct KeyedHasher {
    sum: u64,
    words: usize,
    secret: [u64; WORDS],
}

impl Hasher for KeyedHasher {
    fn write(&mut self, bytes: &[u8]) {
        for chunk in bytes.chunks(8) {
            let mut word = [0u8; 8];
            word[..chunk.len()].copy_from_slice(chunk);
            self.write_u64(u64::from_le_bytes(word));
        }
    }

    fn write_u8(&mut self, value: u8) {
        self.write_u64(u64::from(value));
    }

    fn write_u16(&mut self, value: u16) {
        self.write_u64(u64::from(value));
    }

    fn write_u32(&mut self, value: u32) {
        self.write_u64(u64::from(value));
    }

    fn write_u64(&mut self, value: u64) {
        let secret = self.secret[self.words % WORDS];
        self.words += 1;
        let low = (value as u32).wrapping_add(secret as u32);
        let high = ((value >> 32) as u32).wrapping_add((secret >> 32) as u32);
        self.sum = self.sum.wrapping_add(u64::from(low) * u64::from(high));
    }

    fn write_usize(&mut self, value: usize) {
        self.write_u64(value as u64);
    }

    /// The sum's low bits depend only on the keys' low bits, and a map picks its bucket from
    /// the low bits, so every bit is spread over the others: MurmurHash3's 64-bit finaliser.
    fn finish(&self) -> u64 {
        let mut hash = self.sum;
        hash ^= hash >> 33;
        hash = hash.wrapping_mul(0xff51_afd7_ed55_8ccd);
        hash ^= hash >> 33;
        hash = hash.wrapping_mul(0xc4ce_b9fe_1a85_ec53);
        hash ^ (hash >> 33)
    }
}
