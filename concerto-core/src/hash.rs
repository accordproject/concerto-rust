//! The seeded hasher of the maps keyed by untrusted input
//! (PORTING.md 3.7).
//!
//! Instance keys, and the model names and regex patterns a multi-tenant
//! server takes from its users, must not be hashed with an unseeded or
//! publicly keyed hash: an attacker can craft keys that all collide, making
//! a map quadratic. [`SeededState`] hashes with SipHash-2-4 under
//! process-wide secret keys. On a native target they come from the standard
//! library's OS-seeded [`RandomState`]. On `wasm32-unknown-unknown` the
//! standard library has no entropy source: `RandomState`'s keys are derived
//! from memory addresses, the same in every instantiation of a build and
//! only bumped by one per map, so anyone with the build can compute them.
//! concerto-wasm sets these keys from `crypto.getRandomValues` at
//! instantiation instead, through [`seed_hasher`].
//!
//! Only the maps built on these hashers are covered: `JsObject`
//! (concerto-core-js), every JSON object ([`crate::json::Map`], which is
//! what untrusted JSON text is parsed into, in place of
//! `serde_json::Value`, whose maps use `RandomState`), and the tables below.
//! A `std::collections::HashMap` or `HashSet` with its default hasher, or a
//! `serde_json::Value`, still hashes with `RandomState`: never key one by
//! untrusted input.
//!
//! [`FastSeededState`] is foldhash under a shared seed derived from the same
//! process-wide keys: near-FxHash speed, for the per-lookup tables keyed by
//! user model names (the manager's namespaces, a model file's local types
//! and import short names), where SipHash cost 20-60% on the introspection
//! rows.
//!
//! FxHash stays for the internal tables keyed by identifiers.

use std::collections::hash_map::RandomState;
use std::collections::{HashMap, HashSet};
use std::hash::BuildHasher;
use std::sync::OnceLock;

use foldhash::SharedSeed;
use foldhash::fast::SeedableRandomState;

/// A `HashMap` hashed with [`SeededState`].
pub type SeededHashMap<K, V> = HashMap<K, V, SeededState>;

/// A `HashSet` hashed with [`SeededState`].
pub type SeededHashSet<T> = HashSet<T, SeededState>;

/// A `HashMap` hashed with [`FastSeededState`].
pub type FastSeededHashMap<K, V> = HashMap<K, V, FastSeededState>;

/// The process-wide SipHash keys of every [`SeededState`]: the ones
/// [`seed_hasher`] set, or else keys drawn from the standard library's
/// [`RandomState`] on first use.
static HASH_KEYS: OnceLock<(u64, u64)> = OnceLock::new();

/// Sets the SipHash keys every [`SeededState`] uses, from the host's
/// entropy; `false`, changing nothing, once they are set. A native build
/// needs no call ([`RandomState`] is OS-seeded); on
/// `wasm32-unknown-unknown` its keys are fixed, so concerto-wasm calls this
/// at instantiation with keys from `crypto.getRandomValues`.
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

/// foldhash's shared seed, derived once from the process-wide keys
/// ([`hash_keys`]), so it is seeded from the OS natively and from
/// `crypto.getRandomValues` on WASM ([`seed_hasher`]), never fixed.
static FOLD_SEED: OnceLock<SharedSeed> = OnceLock::new();

/// foldhash (`fast`) seeded from the [`SeededState`] keys ([`seed_hasher`]),
/// for per-lookup tables keyed by model names, where SipHash is too slow.
/// It resists HashDoS from keys chosen without the seed, but is not a keyed
/// PRF, so instance keys keep SipHash. foldhash's own `RandomState` has no
/// entropy on `wasm32-unknown-unknown`.
#[derive(Debug, Clone, Copy)]
pub struct FastSeededState(SeedableRandomState);

impl Default for FastSeededState {
    #[inline]
    fn default() -> Self {
        let (k0, k1) = hash_keys();
        let shared = FOLD_SEED.get_or_init(|| SharedSeed::from_u64(k1));
        Self(SeedableRandomState::with_seed(k0, shared))
    }
}

impl BuildHasher for FastSeededState {
    type Hasher = foldhash::fast::FoldHasher;

    #[inline]
    fn build_hasher(&self) -> Self::Hasher {
        self.0.build_hasher()
    }
}

#[cfg(test)]
mod tests;
