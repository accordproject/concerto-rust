//! Seeds the hasher of the maps keyed by untrusted input, `JsObject`
//! (concerto-core-js, the keys of user-supplied instances) and
//! concerto-core's tables keyed by model names and regex patterns
//! ([`concerto_core::hash::SeededState`]), from the host's entropy.
//!
//! On `wasm32-unknown-unknown` the standard library has no entropy source:
//! `RandomState`'s keys are the same fixed values in every instantiation,
//! so anyone can craft keys that collide under them and make building,
//! validating or serializing an instance quadratic. [`start`] runs when the
//! module is instantiated, before any binding can build a seeded map, and
//! hands concerto-core 128 bits from `crypto.getRandomValues`
//! ([`concerto_core::hash::seed_hasher`]). [`hash_seed`] lets the smokes
//! (scripts/checks.mjs, scripts/node-smoke.cjs) check that it did.

use std::cell::Cell;
use std::hash::BuildHasher;

use concerto_core::hash::SeededState;
use js_sys::{Function, Object, Reflect, Uint8Array};
use wasm_bindgen::prelude::*;

/// Where the hasher's keys came from (see [`hash_seed`]).
#[derive(Clone, Copy)]
enum Source {
    /// `globalThis.crypto.getRandomValues`.
    Crypto,
    /// No `crypto.getRandomValues`: `Math.random`, which is not a CSPRNG
    /// but differs per instantiation, unlike the fixed keys.
    MathRandom,
    /// [`start`] has not run, or the keys were set before it.
    Unseeded,
}

thread_local! {
    /// What [`start`] seeded the hasher from.
    static SOURCE: Cell<Source> = const { Cell::new(Source::Unseeded) };
}

/// 16 bytes from `globalThis.crypto.getRandomValues`, when the host has it
/// (every supported Node, `^20.19.0 || >=22.12.0`, and every browser).
fn crypto_bytes() -> Option<[u8; 16]> {
    let crypto = Reflect::get(&js_sys::global(), &JsValue::from_str("crypto")).ok()?;
    if !crypto.is_object() {
        return None;
    }
    let get_random_values: Function = Reflect::get(&crypto, &JsValue::from_str("getRandomValues"))
        .ok()?
        .dyn_into()
        .ok()?;
    let buffer = Uint8Array::new_with_length(16);
    get_random_values.call1(&crypto, &buffer).ok()?;
    let mut bytes = [0; 16];
    buffer.copy_to(&mut bytes);
    Some(bytes)
}

/// 128 bits from `Math.random`, 32 bits per call.
fn math_random_bytes() -> [u8; 16] {
    (0..4)
        .fold(0_u128, |acc, _| {
            // `Math.random()` is in [0, 1): the product is below 2^32.
            let word = (js_sys::Math::random() * 4_294_967_296.0) as u32;
            (acc << 32) | u128::from(word)
        })
        .to_le_bytes()
}

/// Runs at instantiation (`#[wasm_bindgen(start)]`): seeds the hasher of
/// the untrusted-keyed maps from the host's entropy.
#[wasm_bindgen(start)]
pub fn start() {
    let (bytes, source) = match crypto_bytes() {
        Some(bytes) => (bytes, Source::Crypto),
        None => (math_random_bytes(), Source::MathRandom),
    };
    let bits = u128::from_le_bytes(bytes);
    if concerto_core::hash::seed_hasher(bits as u64, (bits >> 64) as u64) {
        SOURCE.with(|s| s.set(source));
    }
}

/// `{source, probe}`: where the seeded hasher's keys came from (`"crypto"`,
/// `"math-random"` or `"unseeded"`), and the hash of a fixed string under
/// them, as 16 hex digits. Two instantiations seeded from the host have
/// different probes; the probe is one keyed SipHash output, which does not
/// give the keys away.
#[wasm_bindgen(js_name = hashSeed)]
pub fn hash_seed() -> Result<Object, JsValue> {
    let source = match SOURCE.with(Cell::get) {
        Source::Crypto => "crypto",
        Source::MathRandom => "math-random",
        Source::Unseeded => "unseeded",
    };
    let probe = SeededState::default().hash_one("concerto-wasm hash seed probe");
    let out = Object::new();
    Reflect::set(&out, &"source".into(), &source.into())?;
    Reflect::set(&out, &"probe".into(), &format!("{probe:016x}").into())?;
    Ok(out)
}
