//! P5-30 (accordproject/concerto-rust#335): the body of concerto-wasm's
//! `decoratorManagerExtractDecorators` binding, split into stages that run
//! the same on the native target (`src/main.rs`) and inside WASM (the
//! `wasm` feature's exports, driven by `scripts/run-wasm.mjs`). Measure
//! only: nothing here is shipped.
//!
//! Stages, in the binding's order (`to_json` / `to_js` in
//! concerto-wasm/src/lib.rs do the text half of the JSON round trip):
//! - `parse`: `serde_json::from_str` of the `models` text (`to_json`).
//! - `rebuild`: `model_manager_from_asts` (a fresh `ModelManager`, then
//!   `add_model_with_definitions` per model).
//! - `extract`: `dcs::extract_decorators`.
//! - `encode`: `extract_result_to_js`, then `serde_json::to_string` (`to_js`).
//! - `drop`: freeing everything the call built.
//!
//! Plus three microbenchmarks that isolate one suspected cause each:
//! allocator churn, SipHash (serde_json's `preserve_order` maps hash every
//! key with std's `RandomState`) and `memcpy`.

#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic, missing_docs)]

use concerto_core::ModelManager;
use concerto_core::dcs::{self, ExtractOptions, extractor::ExtractResult};
use serde_json::{Value, json};

#[cfg(all(feature = "wasm-talc", target_arch = "wasm32"))]
#[global_allocator]
static ALLOC: talc::TalckWasm = unsafe { talc::TalckWasm::new_global() };

#[cfg(all(feature = "native-dlmalloc", not(target_arch = "wasm32")))]
#[global_allocator]
static ALLOC: dlmalloc::GlobalDlmalloc = dlmalloc::GlobalDlmalloc;

/// The per-call state the stages hand on to each other.
#[derive(Default)]
pub struct State {
    pub input: String,
    pub models: Vec<Value>,
    pub mm: Option<ModelManager>,
    pub result: Option<ExtractResult>,
    pub output: String,
}

pub fn stage_parse(s: &mut State) {
    let v: Value = serde_json::from_str(&s.input).expect("models JSON");
    s.models = match v {
        Value::Array(a) => a,
        _ => panic!("models is an array"),
    };
}

pub fn stage_rebuild(s: &mut State) {
    let mut mm = ModelManager::new().expect("fresh manager");
    for model in &s.models {
        mm.add_model_with_definitions(model, None, None)
            .expect("model adds");
    }
    s.mm = Some(mm);
}

pub fn stage_extract(s: &mut State) {
    let opts = ExtractOptions {
        remove_decorators_from_model: true,
        locale: "en".to_string(),
    };
    s.result =
        Some(dcs::extract_decorators(s.mm.as_ref().expect("manager"), &opts).expect("extract"));
}

pub fn stage_encode(s: &mut State) {
    let result = s.result.take().expect("result");
    let models: Vec<Value> = result
        .model_manager
        .model_files()
        .map(|mf| mf.ast().clone())
        .collect();
    let value = json!({
        "modelManager": { "$class": "concerto.metamodel@1.0.0.Models", "models": models },
        "decoratorCommandSet": result.decorator_command_set,
        "vocabularies": result.vocabularies,
    });
    s.output = serde_json::to_string(&value).expect("encodes");
    // `result` (its manager included) is dropped here, as in the binding.
    s.result = None;
    drop(result);
}

pub fn stage_drop(s: &mut State) {
    s.models = Vec::new();
    s.mm = None;
    s.result = None;
    s.output = String::new();
}

/// Allocator churn: `n` rounds of allocating a small String, a Vec of
/// values and a map, the shapes the DCS path builds, then freeing them.
pub fn micro_alloc(n: u32) -> usize {
    let mut keep = 0usize;
    for i in 0..n {
        let mut m = serde_json::Map::new();
        m.insert(
            "$class".to_string(),
            Value::String(format!("x.y@1.0.0.C{i}")),
        );
        m.insert("name".to_string(), Value::String("name".to_string()));
        // A heap Vec on purpose: the microbenchmark measures allocator churn.
        #[allow(clippy::useless_vec)]
        let v = vec![Value::Object(m.clone()), Value::Object(m)];
        keep += v.len();
    }
    keep
}

/// SipHash-1-3 (std `RandomState`) over `n` short keys.
pub fn micro_siphash(n: u32) -> u64 {
    use std::hash::{BuildHasher, Hasher};
    let st = std::collections::hash_map::RandomState::new();
    let keys = [
        "$class",
        "name",
        "declarations",
        "properties",
        "decorators",
        "namespace",
    ];
    let mut acc = 0u64;
    for i in 0..n {
        let mut h = st.build_hasher();
        h.write(keys[(i as usize) % keys.len()].as_bytes());
        acc ^= h.finish();
    }
    acc
}

/// `n` copies of a 4 KiB buffer.
pub fn micro_memcpy(n: u32) -> usize {
    let src = vec![7u8; 4096];
    let mut dst = vec![0u8; 4096];
    let mut acc = 0usize;
    for i in 0..n {
        dst.copy_from_slice(&src);
        acc += dst[(i as usize) & 4095] as usize;
        std::hint::black_box(&mut dst);
    }
    acc
}

#[cfg(feature = "wasm")]
mod wasm {
    use super::*;
    use std::cell::RefCell;
    use wasm_bindgen::prelude::*;

    thread_local! {
        static S: RefCell<State> = RefCell::new(State::default());
    }

    #[wasm_bindgen]
    pub fn p530_set_input(text: String) {
        S.with(|s| s.borrow_mut().input = text);
    }
    #[wasm_bindgen]
    pub fn p530_parse() {
        S.with(|s| stage_parse(&mut s.borrow_mut()));
    }
    #[wasm_bindgen]
    pub fn p530_rebuild() {
        S.with(|s| stage_rebuild(&mut s.borrow_mut()));
    }
    #[wasm_bindgen]
    pub fn p530_extract() {
        S.with(|s| stage_extract(&mut s.borrow_mut()));
    }
    #[wasm_bindgen]
    pub fn p530_encode() {
        S.with(|s| stage_encode(&mut s.borrow_mut()));
    }
    #[wasm_bindgen]
    pub fn p530_take_output() -> String {
        S.with(|s| std::mem::take(&mut s.borrow_mut().output))
    }
    #[wasm_bindgen]
    pub fn p530_drop() {
        S.with(|s| stage_drop(&mut s.borrow_mut()));
    }
    /// The whole binding body in one call (input text in, output text out).
    #[wasm_bindgen]
    pub fn p530_all(text: String) -> String {
        let mut s = State {
            input: text,
            ..State::default()
        };
        stage_parse(&mut s);
        stage_rebuild(&mut s);
        stage_extract(&mut s);
        stage_encode(&mut s);
        std::mem::take(&mut s.output)
    }
    #[wasm_bindgen]
    pub fn p530_micro_alloc(n: u32) -> usize {
        micro_alloc(n)
    }
    #[wasm_bindgen]
    pub fn p530_micro_siphash(n: u32) -> u64 {
        micro_siphash(n)
    }
    #[wasm_bindgen]
    pub fn p530_micro_memcpy(n: u32) -> usize {
        micro_memcpy(n)
    }
    /// Current linear memory size in bytes.
    #[wasm_bindgen]
    pub fn p530_memory_bytes() -> usize {
        core::arch::wasm32::memory_size(0) * 65536
    }
}
