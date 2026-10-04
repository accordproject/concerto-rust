//! The seeded hasher of the maps keyed by untrusted input
//! (PORTING.md 3.7).
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
mod tests {
    use super::*;

    /// `count` distinct 48-byte keys that all have the same FxHash
    /// (rustc-hash 2, 64-bit targets): the word at 32 repeats the first
    /// chunk's mix `t1`, which zeroes the final multiply (see
    /// concerto-core-js/tests/hashdos.rs, which times the instance path).
    #[cfg(target_pointer_width = "64")]
    fn fx_colliding(count: usize) -> Vec<String> {
        fn multiply_mix(x: u64, y: u64) -> u64 {
            let full = u128::from(x).wrapping_mul(u128::from(y));
            (full as u64) ^ ((full >> 64) as u64)
        }
        fn le(bytes: &[u8]) -> u64 {
            let mut word = [0; 8];
            word.copy_from_slice(&bytes[..8]);
            u64::from_le_bytes(word)
        }
        const SEED1: u64 = 0x243f_6a88_85a3_08d3;
        const PREVENT_TRIVIAL_ZERO_COLLAPSE: u64 = 0xa409_3822_299f_31d0;
        let printable = |word: u64| word.to_le_bytes().iter().all(|b| (0x20..0x7f).contains(b));
        let (prefix, t1) = (0u64..)
            .map(|n| format!("hashdos-{n:08}"))
            .map(|prefix| {
                let bytes = prefix.as_bytes();
                let t1 = multiply_mix(
                    SEED1 ^ le(&bytes[..8]),
                    PREVENT_TRIVIAL_ZERO_COLLAPSE ^ le(&bytes[8..]),
                );
                (prefix, t1)
            })
            .find(|(_, t1)| printable(*t1))
            .expect("a printable t1");
        let cancel = String::from_utf8(t1.to_le_bytes().to_vec()).expect("printable ASCII");
        (0..count)
            .map(|i| format!("{prefix}{i:016x}{cancel}--------"))
            .collect()
    }

    #[test]
    fn fast_seeded_state_is_foldhash_under_the_process_keys() {
        let (k0, k1) = hash_keys();
        let shared: &'static SharedSeed = Box::leak(Box::new(SharedSeed::from_u64(k1)));
        let expected = SeedableRandomState::with_seed(k0, shared);
        let state = FastSeededState::default();
        for key in ["a", "org.example@1.0.0", "Person"] {
            assert_eq!(state.hash_one(key), expected.hash_one(key));
        }
        // Not foldhash's fixed (public) seed.
        assert_ne!(
            state.hash_one("org.example@1.0.0"),
            foldhash::fast::FixedState::default().hash_one("org.example@1.0.0")
        );
    }

    /// The model-name tables (`ModelManager`'s namespaces, `ModelFile`'s
    /// local types and import short names) do not inherit FxHash's
    /// collisions: names crafted to share one FxHash hash apart.
    #[cfg(target_pointer_width = "64")]
    #[test]
    fn fx_colliding_names_hash_apart() {
        const NAMES: usize = 2000;
        let names = fx_colliding(NAMES);
        let fx = rustc_hash::FxBuildHasher;
        let first = fx.hash_one(names[0].as_str());
        assert!(
            names.iter().all(|n| fx.hash_one(n.as_str()) == first),
            "the crafted names no longer collide under FxHash: update fx_colliding()"
        );
        let fast = FastSeededState::default();
        let distinct: HashSet<u64> = names.iter().map(|n| fast.hash_one(n.as_str())).collect();
        assert_eq!(distinct.len(), NAMES);
        let seeded = SeededState::default();
        let distinct: HashSet<u64> = names.iter().map(|n| seeded.hash_one(n.as_str())).collect();
        assert_eq!(distinct.len(), NAMES);
    }
}
