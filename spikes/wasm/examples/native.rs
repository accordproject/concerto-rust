//! The same load / validate work as the WASM benchmark, run natively, so the
//! report can separate WASM overhead from the cost of the algorithm itself.
//!
//!     node scripts/dump-set.mjs
//!     cargo run --release --example native
//!     cargo run --profile release-speed --example native

use std::time::{Duration, Instant};

use concerto_core::ModelManager;

/// Best-of-N time per iteration, running for at least `min`.
fn best(min: Duration, mut f: impl FnMut()) -> f64 {
    let mut best = f64::MAX;
    let start = Instant::now();
    while start.elapsed() < min {
        let t = Instant::now();
        f();
        best = best.min(t.elapsed().as_secs_f64() * 1e3);
    }
    best
}

fn main() {
    let text =
        std::fs::read_to_string("dist/bench-set.json").expect("run scripts/dump-set.mjs first");
    let set: Vec<serde_json::Value> = serde_json::from_str(&text).unwrap();
    let strings: Vec<String> = set.iter().map(|m| m.to_string()).collect();
    let min = Duration::from_secs(2);

    let load = best(min, || {
        let mut mgr = ModelManager::new().unwrap();
        for s in &strings {
            let v: serde_json::Value = serde_json::from_str(s).unwrap();
            mgr.add_model(&v, None).unwrap();
        }
    });
    let parse_only = best(min, || {
        for s in &strings {
            let _: serde_json::Value = serde_json::from_str(s).unwrap();
        }
    });
    let mut mgr = ModelManager::new().unwrap();
    for v in &set {
        mgr.add_model(v, None).unwrap();
    }
    let validate = best(min, || mgr.validate_models().unwrap());

    println!("native load set (serde_json parse + add_model): {load:.3} ms");
    println!("native serde_json parse only:                    {parse_only:.3} ms");
    println!("native validate_models:                          {validate:.3} ms");
}
