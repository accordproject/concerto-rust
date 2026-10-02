//! P5-90 (accordproject/concerto-rust#436), Phase 0, measure only: the
//! native typed read that the WASM binding `stageModelFileChecked(Utf8)`
//! runs per `new ModelFile` (`ModelFile::from_json_text_checked_with_imports`:
//! BC-19's folded shape check and the strict typed load, from one parse),
//! over the P5-15 sweep's inputs (`migration/bench/fixtures/p515/<set>.json`
//! in the concerto repo).
//!
//! ```text
//! cargo run --release --example p590_typed_read -- time  <p515 dir>
//! cargo run --release --example p590_typed_read -- alloc <p515 dir>
//! cargo run --release --example p590_typed_read -- loop  <p515 dir> <set> <iterations>
//! ```
//!
//! - `time`: the median µs per model file (40 passes, the first 10 dropped).
//! - `alloc`: heap allocations, reallocations and bytes per model file,
//!   counted by this example's global allocator (which only counts and
//!   forwards to the system allocator).
//! - `loop`: one set's typed reads a fixed number of times, for
//!   `valgrind --tool=dhat` (the allocation sites; see
//!   `migration/bench/p590-dhat.mjs` in the concerto repo). Build with
//!   `CARGO_PROFILE_RELEASE_DEBUG=true` for symbolised frames.

#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use std::alloc::{GlobalAlloc, Layout, System};
use std::hint::black_box;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

use concerto_core::ModelFile;
use serde_json::Value;

struct Counting;

static ALLOCS: AtomicU64 = AtomicU64::new(0);
static REALLOCS: AtomicU64 = AtomicU64::new(0);
static BYTES: AtomicU64 = AtomicU64::new(0);

// SAFETY: forwards every call to the system allocator unchanged, only
// counting the allocations.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        ALLOCS.fetch_add(1, Ordering::Relaxed);
        BYTES.fetch_add(layout.size() as u64, Ordering::Relaxed);
        // SAFETY: the caller's contract is System's.
        unsafe { System.alloc(layout) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        // SAFETY: the caller's contract is System's.
        unsafe { System.dealloc(ptr, layout) }
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        REALLOCS.fetch_add(1, Ordering::Relaxed);
        BYTES.fetch_add(new_size as u64, Ordering::Relaxed);
        // SAFETY: the caller's contract is System's.
        unsafe { System.realloc(ptr, layout, new_size) }
    }
}

#[global_allocator]
static GLOBAL: Counting = Counting;

const SETS: [&str; 3] = ["concerto-core-test-data", "conformance", "synthetic-large"];

/// A set's model files, as `(file name, AST JSON text)`, with the
/// enum `isAbstract` key dropped as `p515-sweep.mjs` drops it.
fn load(dir: &str, set: &str) -> Vec<(String, String)> {
    let text = std::fs::read_to_string(format!("{dir}/{set}.json")).expect("p515 fixture");
    let v: Value = serde_json::from_str(&text).expect("p515 fixture JSON");
    v["models"]
        .as_array()
        .expect("models")
        .iter()
        .map(|m| {
            let mut ast = m["ast"].clone();
            if let Some(decls) = ast.get_mut("declarations").and_then(Value::as_array_mut) {
                for d in decls {
                    if d["$class"] == "concerto.metamodel@1.0.0.EnumDeclaration" {
                        d.as_object_mut().expect("declaration").remove("isAbstract");
                    }
                }
            }
            (
                m["name"].as_str().expect("name").to_string(),
                serde_json::to_string(&ast).expect("AST text"),
            )
        })
        .collect()
}

#[inline(never)]
fn typed_read(models: &[(String, String)]) {
    for (name, text) in models {
        let loaded = ModelFile::from_json_text_checked_with_imports(text, None, Some(name.clone()))
            .expect("AST text parses")
            .expect("AST loads");
        black_box(loaded);
    }
}

fn median(mut v: Vec<f64>) -> f64 {
    v.sort_by(f64::total_cmp);
    v[v.len() / 2]
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let dir = args.get(2).expect("p515 dir");
    match args.get(1).map(String::as_str) {
        Some("time") => {
            for set in SETS {
                let models = load(dir, set);
                let n = models.len() as f64;
                let mut v = Vec::new();
                for s in 0..40 {
                    let t0 = Instant::now();
                    typed_read(&models);
                    if s >= 10 {
                        v.push(t0.elapsed().as_secs_f64() * 1e6 / n);
                    }
                }
                println!("typed_read\t{set}\t{:.2}", median(v));
            }
        }
        Some("alloc") => {
            for set in SETS {
                let models = load(dir, set);
                typed_read(&models);
                let n = models.len() as f64;
                let (a0, r0, b0) = (
                    ALLOCS.load(Ordering::Relaxed),
                    REALLOCS.load(Ordering::Relaxed),
                    BYTES.load(Ordering::Relaxed),
                );
                typed_read(&models);
                let (a1, r1, b1) = (
                    ALLOCS.load(Ordering::Relaxed),
                    REALLOCS.load(Ordering::Relaxed),
                    BYTES.load(Ordering::Relaxed),
                );
                let text: usize = models.iter().map(|(_, t)| t.len()).sum();
                println!(
                    "typed_read\t{set}\tallocs {:.0}\treallocs {:.0}\tbytes {:.0}\ttext bytes {:.0}",
                    (a1 - a0) as f64 / n,
                    (r1 - r0) as f64 / n,
                    (b1 - b0) as f64 / n,
                    text as f64 / n
                );
            }
        }
        Some("loop") => {
            let models = load(dir, args.get(3).expect("set"));
            let n: u64 = args
                .get(4)
                .expect("iterations")
                .parse()
                .expect("iterations");
            for _ in 0..n {
                typed_read(&models);
            }
        }
        _ => panic!("usage: p590_typed_read time|alloc <p515 dir> | loop <p515 dir> <set> <n>"),
    }
}
