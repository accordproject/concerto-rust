//! The key source of the WASM HashDoS check (scripts/hashdos.mjs): object
//! keys crafted to collide under the standard library's hasher as it is on
//! `wasm32-unknown-unknown`, and a control map that shows they do.
//!
//! On that target `RandomState` has no entropy: its keys come from memory
//! addresses, the same in every instantiation of a build, and are only
//! bumped by one per map. Anyone with the build can compute them, as this
//! module does with its own: [`std_keys`] picks keys whose hashes under one
//! `RandomState` share their low bits, so they all start probing at the same
//! bucket of a hashbrown table (`std::collections::HashMap`, and the
//! `IndexMap` behind `serde_json::Map`) up to that size. [`std_insert`]
//! inserts keys into a map with that very hasher: quadratic for the crafted
//! keys. The check then hands the same keys to the engine's entry points,
//! whose maps are seeded (`concerto_core::json`), and expects them to cost
//! about what ordinary keys of the same shape do.
//!
//! Built for `wasm32-unknown-unknown` only, with wasm-bindgen's glue.

use std::cell::RefCell;
use std::collections::HashMap;
use std::collections::hash_map::RandomState;
use std::hash::BuildHasher;

use wasm_bindgen::prelude::*;

thread_local! {
    /// The fixed-key hasher the keys are crafted against.
    static STATE: RefCell<Option<RandomState>> = const { RefCell::new(None) };
}

fn with_state<T>(f: impl FnOnce(&RandomState) -> T) -> T {
    STATE.with(|state| f(state.borrow_mut().get_or_insert_with(RandomState::new)))
}

/// `count` distinct keys `hashdos-<n>` (zero-padded). With `collide`, only
/// keys whose hash under [`STATE`] has its low `bits` bits zero are kept,
/// so they collide in every hashbrown table of up to `2^bits` buckets;
/// without it, every key is kept.
#[wasm_bindgen(js_name = stdKeys)]
pub fn std_keys(count: u32, bits: u32, collide: bool) -> Vec<String> {
    let mask = (1_u64 << bits) - 1;
    with_state(|state| {
        (0_u64..)
            .map(|n| format!("hashdos-{n:012}"))
            .filter(|key| !collide || state.hash_one(key.as_str()) & mask == 0)
            .take(count as usize)
            .collect()
    })
}

/// Inserts `keys` into a `HashMap` keyed by [`STATE`], the hasher they were
/// crafted against, and returns its length.
#[wasm_bindgen(js_name = stdInsert)]
pub fn std_insert(keys: Vec<String>) -> usize {
    with_state(|state| {
        let mut map = HashMap::with_hasher(state.clone());
        for key in keys {
            map.insert(key, ());
        }
        map.len()
    })
}
