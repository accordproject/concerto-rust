//! The WASM HashDoS check's key source (scripts/hashdos.mjs), built only
//! with the `hashdos-probe` feature: object keys crafted to collide under
//! the standard library's hasher as it is on `wasm32-unknown-unknown`.
//!
//! On that target `RandomState` has no entropy: its keys are derived from
//! memory addresses, the same in every instantiation of a build, and the
//! first is bumped by one for each new map. Anyone with the build can
//! compute them; this module reads them from a fresh `RandomState` (checked
//! against its own SipHash-1-3, std's `DefaultHasher`), so [`std_keys`] can
//! pick keys for the hasher of a map that has not been built yet: the next
//! one, or the one `delta` maps after it. Their hashes share their low
//! `bits` bits, so they all start probing at the same bucket of a hashbrown
//! table (`std::collections::HashMap`, and the `IndexMap` behind
//! `serde_json::Map`) of up to `2^bits` buckets, and inserting them is
//! quadratic. [`std_insert`] shows that for a std map; the check then hands
//! keys crafted for the next map to the engine's entry points, which would
//! be just as quadratic if they parsed into `serde_json::Value`.
//!
//! The keys it reads are the ones anyone with the build can compute; the
//! feature only saves the check from computing them from the outside.

use std::collections::HashMap;
use std::collections::hash_map::RandomState;
use std::hash::{BuildHasher, Hasher};

use wasm_bindgen::prelude::*;

/// SipHash-1-3 under `(k0, k1)`: the standard library's `DefaultHasher`,
/// which `RandomState` builds.
#[derive(Clone, Copy)]
struct Sip13 {
    v: [u64; 4],
    tail: u64,
    ntail: u32,
    length: u64,
}

impl Sip13 {
    fn new(k0: u64, k1: u64) -> Self {
        Self {
            v: [
                k0 ^ 0x736f_6d65_7073_6575,
                k1 ^ 0x646f_7261_6e64_6f6d,
                k0 ^ 0x6c79_6765_6e65_7261,
                k1 ^ 0x7465_6462_7974_6573,
            ],
            tail: 0,
            ntail: 0,
            length: 0,
        }
    }

    fn round(v: &mut [u64; 4]) {
        let [v0, v1, v2, v3] = v;
        *v0 = v0.wrapping_add(*v1);
        *v1 = v1.rotate_left(13) ^ *v0;
        *v0 = v0.rotate_left(32);
        *v2 = v2.wrapping_add(*v3);
        *v3 = v3.rotate_left(16) ^ *v2;
        *v0 = v0.wrapping_add(*v3);
        *v3 = v3.rotate_left(21) ^ *v0;
        *v2 = v2.wrapping_add(*v1);
        *v1 = v1.rotate_left(17) ^ *v2;
        *v2 = v2.rotate_left(32);
    }

    fn compress(&mut self, m: u64) {
        self.v[3] ^= m;
        Self::round(&mut self.v);
        self.v[0] ^= m;
    }
}

impl Hasher for Sip13 {
    fn write(&mut self, bytes: &[u8]) {
        for &b in bytes {
            self.tail |= u64::from(b) << (8 * self.ntail);
            self.ntail += 1;
            self.length += 1;
            if self.ntail == 8 {
                let m = self.tail;
                self.compress(m);
                self.tail = 0;
                self.ntail = 0;
            }
        }
    }

    fn finish(&self) -> u64 {
        let mut s = *self;
        s.compress(((self.length & 0xff) << 56) | self.tail);
        s.v[2] ^= 0xff;
        for _ in 0..3 {
            Self::round(&mut s.v);
        }
        s.v[0] ^ s.v[1] ^ s.v[2] ^ s.v[3]
    }
}

/// `key`'s hash under SipHash-1-3 with `keys`, as `RandomState` hashes a
/// `str` (its bytes, then `0xff`).
fn sip13(keys: (u64, u64), key: &str) -> u64 {
    let mut hasher = Sip13::new(keys.0, keys.1);
    hasher.write(key.as_bytes());
    hasher.write_u8(0xff);
    hasher.finish()
}

/// The keys of a new `RandomState`, read from its two private `u64`
/// fields and checked: the state must hash a probe string as [`sip13`]
/// does with them. Throws when the standard library no longer matches.
fn next_std_keys() -> Result<(u64, u64), JsError> {
    let state = RandomState::new();
    let probe = "concerto-wasm hashdos probe";
    let expected = state.hash_one(probe);
    // SAFETY: `RandomState` is two `u64`s (`k0`, `k1`); any bits are a
    // valid `[u64; 2]`. Their order is unspecified, so both are tried, and
    // a match is checked by hashing.
    let [a, b] = unsafe { std::mem::transmute::<RandomState, [u64; 2]>(state) };
    [(a, b), (b, a)]
        .into_iter()
        .find(|&keys| sip13(keys, probe) == expected)
        .ok_or_else(|| JsError::new("std's RandomState is no longer SipHash-1-3 over two u64 keys"))
}

/// `count` distinct keys `hashdos-<n>` whose hashes have their low `bits`
/// bits zero under the hasher of the map built `delta` maps after the next
/// one (`delta` 0: the next map). Reading the keys builds a `RandomState`
/// itself, so the next map is the first one built after this call.
#[wasm_bindgen(js_name = stdKeys)]
pub fn std_keys(count: u32, bits: u32, delta: u32) -> Result<Vec<String>, JsError> {
    let (k0, k1) = next_std_keys()?;
    let target = (k0.wrapping_add(1 + u64::from(delta)), k1);
    let mask = 1_u64.checked_shl(bits).map_or(u64::MAX, |bit| bit - 1);
    Ok((0_u64..)
        .map(|n| format!("hashdos-{n:012}"))
        .filter(|key| sip13(target, key) & mask == 0)
        .take(count as usize)
        .collect())
}

/// `count` keys of the same shape as [`std_keys`]'s, not picked.
#[wasm_bindgen(js_name = ordinaryKeys)]
pub fn ordinary_keys(count: u32) -> Vec<String> {
    (0..u64::from(count))
        .map(|n| format!("hashdos-{n:012}"))
        .collect()
}

/// Inserts `keys` into a new std `HashMap` (the next map) and returns its
/// length.
#[wasm_bindgen(js_name = stdInsert)]
pub fn std_insert(keys: Vec<String>) -> usize {
    let mut map = HashMap::new();
    for key in keys {
        map.insert(key, ());
    }
    map.len()
}
