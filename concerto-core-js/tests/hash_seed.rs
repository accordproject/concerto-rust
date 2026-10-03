//! P5-110 (accordproject/concerto-rust#477): `concerto_core::hash::seed_hasher`,
//! the hook concerto-wasm seeds the hasher of `JsObject` and of concerto-core's
//! untrusted-keyed tables through at instantiation
//! (concerto-wasm/src/hash_seed.rs), since `RandomState` has fixed keys on
//! `wasm32-unknown-unknown`. Its own test binary: the keys are process-wide
//! and set once.

use std::hash::BuildHasher;

use concerto_core::hash::{SeededState, seed_hasher};
use concerto_core_js::{JsObject, JsValue};

#[expect(deprecated, reason = "the keyed SipHash SeededState builds")]
fn sip(k0: u64, k1: u64, key: &str) -> u64 {
    let mut hasher = std::hash::SipHasher::new_with_keys(k0, k1);
    std::hash::Hash::hash(key, &mut hasher);
    std::hash::Hasher::finish(&hasher)
}

#[test]
fn seeded_keys_are_used_and_set_once() {
    let (k0, k1) = (0x0123_4567_89ab_cdef, 0xfedc_ba98_7654_3210);
    assert!(seed_hasher(k0, k1), "the first seed is taken");
    let state = SeededState::default();
    assert_eq!(state.hash_one("key"), sip(k0, k1, "key"));
    assert_ne!(state.hash_one("key"), sip(0, 0, "key"));

    // Once set, the keys stay: a map built earlier still finds its keys.
    let mut map = JsObject::default();
    map.insert("a".into(), JsValue::Null);
    assert!(!seed_hasher(1, 2), "a second seed is refused");
    assert_eq!(SeededState::default().hash_one("key"), sip(k0, k1, "key"));
    assert!(map.contains_key("a"));
}
