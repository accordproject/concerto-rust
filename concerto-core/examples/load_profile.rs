//! Native timing, allocation counts and profiling loops for model loading
//! (accordproject/concerto-rust#369, P5-48): the engine side of `new
//! ModelFile`, `addModelFile`, `addCTOModel` and `new ModelManager`, over
//! the P5-15 sweep's inputs (`migration/bench/fixtures/p515/<set>.json` in
//! the concerto repo), as the `p515_sweep` bench's load rows run them.
//!
//! ```text
//! cargo run --release -p accordproject-concerto-core --example load_profile -- time <p515 dir>
//! cargo run --release -p accordproject-concerto-core --example load_profile -- alloc <p515 dir>
//! cargo run --release -p accordproject-concerto-core --example load_profile -- loop <p515 dir> <op> <set> <iterations>
//! ```
//!
//! - `time` prints the median µs per model file of each op and set, and of
//!   each stage of `add_model_file` (build; validation and registration).
//! - `alloc` prints heap allocations and bytes allocated per model file for
//!   the same rows, counted by this example's global allocator.
//! - `loop` runs one op a fixed number of times, for a profiler such as
//!   `valgrind --tool=callgrind` (the P5-12d `sample` profiles are macOS
//!   only).
//!
//! The ops: `mm_new` (`ModelManager::new`); `modelfile_new`
//! (`ModelFile::from_json_text`, what `stageModelFileBytes` runs without
//! the shape check);
//! `add_model_file` (a fresh manager, then per file `from_json_text`,
//! `validate_and_add_model_file`: the stage, then the validate and
//! commit of `addModelFile`/`addCTOModel`). `addCTOModel`'s
//! CTO parse is concerto-cto (TS) on both engines, so it has no row here.

#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use std::alloc::{GlobalAlloc, Layout, System};
use std::hint::black_box;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

use concerto_core::{ModelFile, ModelManager};
use serde_json::Value;

struct Counting;

static ALLOCS: AtomicU64 = AtomicU64::new(0);
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
        ALLOCS.fetch_add(1, Ordering::Relaxed);
        BYTES.fetch_add(new_size as u64, Ordering::Relaxed);
        // SAFETY: the caller's contract is System's.
        unsafe { System.realloc(ptr, layout, new_size) }
    }
}

#[global_allocator]
static GLOBAL: Counting = Counting;

const SETS: [&str; 3] = ["concerto-core-test-data", "conformance", "synthetic-large"];

/// A set's model files, as `(file name, AST JSON text)`.
fn load(dir: &str, set: &str) -> Vec<(String, String)> {
    let text = std::fs::read_to_string(format!("{dir}/{set}.json")).expect("p515 fixture");
    let v: Value = serde_json::from_str(&text).expect("p515 fixture JSON");
    v["models"]
        .as_array()
        .expect("models")
        .iter()
        .map(|m| {
            (
                m["name"].as_str().expect("name").to_string(),
                serde_json::to_string(&m["ast"]).expect("AST text"),
            )
        })
        .collect()
}

fn file_of(name: &str, text: &str) -> ModelFile {
    ModelFile::from_json_text(text, None, Some(name.to_string()))
        .expect("AST text parses")
        .expect("AST loads")
}

#[inline(never)]
fn modelfile_new(models: &[(String, String)]) {
    for (name, text) in models {
        black_box(file_of(name, text));
    }
}

#[inline(never)]
fn add_model_file(models: &[(String, String)]) -> ModelManager {
    let mut mm = ModelManager::new().expect("a fresh manager");
    for (name, text) in models {
        let mf = file_of(name, text);
        add_one(&mut mm, mf);
    }
    mm
}

/// Validates and registers one file, as the binding's
/// `validateAndCommitStagedModelFile` does.
fn add_one(mm: &mut ModelManager, mf: ModelFile) {
    mm.validate_and_add_model_file(mf)
        .map_err(|(err, _)| err)
        .expect("model validates and adds");
}

/// `add_model_file`, timing (or counting) each stage separately:
/// `[build, validate and register]`, summed over the files.
fn add_model_file_stages(models: &[(String, String)], measure: &dyn Fn() -> f64) -> [f64; 2] {
    let mut out = [0.0; 2];
    let mut mm = ModelManager::new().expect("a fresh manager");
    for (name, text) in models {
        let t0 = measure();
        let mf = file_of(name, text);
        let t1 = measure();
        add_one(&mut mm, mf);
        let t2 = measure();
        out[0] += t1 - t0;
        out[1] += t2 - t1;
    }
    black_box(mm);
    out
}

#[inline(never)]
fn mm_new() {
    black_box(ModelManager::new().expect("a fresh manager"));
}

fn median(mut v: Vec<f64>) -> f64 {
    v.sort_by(f64::total_cmp);
    v[v.len() / 2]
}

fn time(dir: &str) {
    let start = Instant::now();
    let now = move || start.elapsed().as_secs_f64() * 1e6;
    let samples = |f: &mut dyn FnMut() -> f64| {
        let mut v = Vec::new();
        for s in 0..40 {
            let x = f();
            if s >= 10 {
                v.push(x);
            }
        }
        median(v)
    };
    let t = samples(&mut || {
        let t0 = now();
        black_box(ModelManager::new().expect("a fresh manager"));
        now() - t0
    });
    println!("mm_new\t-\t{t:.2}");
    for set in SETS {
        let models = load(dir, set);
        let n = models.len() as f64;
        let t = samples(&mut || {
            let t0 = now();
            modelfile_new(&models);
            (now() - t0) / n
        });
        println!("modelfile_new\t{set}\t{t:.2}");
        let t = samples(&mut || {
            let t0 = now();
            black_box(add_model_file(&models));
            (now() - t0) / n
        });
        println!("add_model_file\t{set}\t{t:.2}");
        let mut parts = [Vec::new(), Vec::new()];
        for s in 0..40 {
            let x = add_model_file_stages(&models, &now);
            if s >= 10 {
                for (part, x) in parts.iter_mut().zip(x) {
                    part.push(x / n);
                }
            }
        }
        let [a, b] = parts.map(median);
        println!("add_model_file.build\t{set}\t{a:.2}");
        println!("add_model_file.validate_register\t{set}\t{b:.2}");
    }
}

fn counts() -> (u64, u64) {
    (
        ALLOCS.load(Ordering::Relaxed),
        BYTES.load(Ordering::Relaxed),
    )
}

fn alloc(dir: &str) {
    // Warm the per-thread system model cache first, as a resident engine is.
    black_box(ModelManager::new().expect("a fresh manager"));
    let (a0, b0) = counts();
    black_box(ModelManager::new().expect("a fresh manager"));
    let (a1, b1) = counts();
    println!("mm_new\t-\t{}\t{}", a1 - a0, b1 - b0);
    for set in SETS {
        let models = load(dir, set);
        let n = models.len() as u64;
        let (a0, b0) = counts();
        modelfile_new(&models);
        let (a1, b1) = counts();
        println!("modelfile_new\t{set}\t{}\t{}", (a1 - a0) / n, (b1 - b0) / n);
        let (a0, b0) = counts();
        black_box(add_model_file(&models));
        let (a1, b1) = counts();
        println!(
            "add_model_file\t{set}\t{}\t{}",
            (a1 - a0) / n,
            (b1 - b0) / n
        );
        let allocs = add_model_file_stages(&models, &|| counts().0 as f64);
        let bytes = add_model_file_stages(&models, &|| counts().1 as f64);
        for (i, stage) in ["build", "validate_register"].iter().enumerate() {
            println!(
                "add_model_file.{stage}\t{set}\t{:.0}\t{:.0}",
                allocs[i] / n as f64,
                bytes[i] / n as f64
            );
        }
    }
}

fn hot_loop(dir: &str, op: &str, set: &str, iterations: u64) {
    let models = if op == "mm_new" {
        Vec::new()
    } else {
        load(dir, set)
    };
    black_box(ModelManager::new().expect("a fresh manager"));
    for _ in 0..iterations {
        match op {
            "mm_new" => mm_new(),
            "modelfile_new" => modelfile_new(&models),
            "add_model_file" => {
                black_box(add_model_file(&models));
            }
            _ => panic!("unknown op {op}"),
        }
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    match args.get(1).map(String::as_str) {
        Some("time") => time(&args[2]),
        Some("alloc") => alloc(&args[2]),
        Some("loop") => hot_loop(
            &args[2],
            &args[3],
            &args[4],
            args[5].parse().expect("iterations"),
        ),
        _ => panic!("usage: load_profile time|alloc <p515 dir> | loop <p515 dir> <op> <set> <n>"),
    }
}
