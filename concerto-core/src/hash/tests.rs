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
    let keyed = SeededState::default();
    let per_hasher = keyed.hash_one(("foldhash", 0_u8));
    let shared = SharedSeed::from_u64(keyed.hash_one(("foldhash", 1_u8)));
    let shared: &'static SharedSeed = Box::leak(Box::new(shared));
    let expected = SeedableRandomState::with_seed(per_hasher, shared);
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

/// foldhash is seeded through a derivation, never with the SipHash keys
/// themselves, so its seeds give nothing away about the keyed tables.
#[test]
fn fold_seeds_are_not_the_siphash_keys() {
    let (k0, k1) = hash_keys();
    let (per_hasher, _) = fold_seed();
    assert!(![k0, k1].contains(per_hasher));
    let raw_keys: &'static SharedSeed = Box::leak(Box::new(SharedSeed::from_u64(k1)));
    let raw = SeedableRandomState::with_seed(k0, raw_keys);
    assert_ne!(
        FastSeededState::default().hash_one("org.example@1.0.0"),
        raw.hash_one("org.example@1.0.0")
    );
}
