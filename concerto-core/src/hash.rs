//! The seeded hasher of the maps keyed by untrusted input (P5-110,
//! accordproject/concerto-rust#477; PORTING.md 3.7).
//!
//! Instance keys, and the model names and regex patterns a multi-tenant
//! server takes from its users, must not be hashed with an unseeded or
//! publicly keyed hash: an attacker can craft keys that all collide, making
//! a map quadratic. [`SeededState`] hashes with SipHash-2-4 under
//! process-wide secret keys. On a native target they come from the standard
//! library's OS-seeded [`RandomState`]. On `wasm32-unknown-unknown` the
//! standard library has no entropy source and `RandomState`'s keys are the
//! same in every instantiation, so concerto-wasm sets them from
//! `crypto.getRandomValues` at instantiation, through [`seed_hasher`].
//!
//! FxHash stays for the internal tables keyed by identifiers.

use std::collections::hash_map::RandomState;
use std::collections::{HashMap, HashSet};
use std::hash::BuildHasher;
use std::sync::OnceLock;

/// A `HashMap` hashed with [`SeededState`].
pub type SeededHashMap<K, V> = HashMap<K, V, SeededState>;

/// A `HashSet` hashed with [`SeededState`].
pub type SeededHashSet<T> = HashSet<T, SeededState>;

/// The process-wide SipHash keys of every [`SeededState`]: the ones
/// [`seed_hasher`] set, or else keys drawn from the standard library's
/// [`RandomState`] on first use.
static HASH_KEYS: OnceLock<(u64, u64)> = OnceLock::new();

/// Sets the SipHash keys every [`SeededState`] hashes with, from the host's
/// entropy. Returns `false`, and changes nothing, once the keys are set: by
/// an earlier call, or by the first [`SeededState`] built.
///
/// A native build needs no call: the keys then come from [`RandomState`],
/// which the OS seeds. On `wasm32-unknown-unknown` `RandomState`'s keys are
/// fixed, so they would be public; concerto-wasm calls this at
/// instantiation, before any seeded map exists, with keys from
/// `crypto.getRandomValues`.
#[cfg_attr(not(feature = "js-compat"), expect(dead_code, reason = "js-compat seam only"))]
pub fn seed_hasher(k0: u64, k1: u64) -> bool {
    HASH_KEYS.set((k0, k1)).is_ok()
}

/// The keys [`SeededState`] uses, set on first use when [`seed_hasher`]
/// has not set them: two outputs of the OS-seeded [`RandomState`].
fn hash_keys() -> (u64, u64) {
    *HASH_KEYS.get_or_init(|| {
        let state = RandomState::new();
        (state.hash_one(0_u64), state.hash_one(1_u64))
    })
}

/// SipHash-2-4 under the process-wide secret keys ([`seed_hasher`]). It
/// differs from the standard library's [`RandomState`] in where the keys
/// come from, which a WASM host can supply. `std::hash::SipHasher`,
/// deprecated only in favour of `DefaultHasher`, is the standard library's
/// one SipHash that takes keys.
#[derive(Debug, Clone, Copy)]
pub struct SeededState {
    k0: u64,
    k1: u64,
}

impl Default for SeededState {
    fn default() -> Self {
        let (k0, k1) = hash_keys();
        Self { k0, k1 }
    }
}

#[expect(deprecated, reason = "SipHasher is the only keyed SipHash in std")]
impl BuildHasher for SeededState {
    type Hasher = std::hash::SipHasher;

    fn build_hasher(&self) -> Self::Hasher {
        std::hash::SipHasher::new_with_keys(self.k0, self.k1)
    }
}
