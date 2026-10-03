//! P5-30: the native half. Times each stage of the extractDecorators
//! binding body (see lib.rs) over the same input text the WASM half uses.
//!
//!   p530-native <models.json> [--iters N] [--warmup N] [--loop STAGE --seconds S]
//!
//! Prints one JSON object: per stage, the median and min in microseconds
//! over `--iters` calls (after `--warmup`), plus the three microbenchmarks.
//! With the `count-alloc` feature it also prints allocations per stage.
//! `--loop all --seconds S` just runs the whole body for S seconds, for a
//! profiler (valgrind --tool=callgrind).

#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic, missing_docs)]

use p530_wasm_profile::*;
use std::hint::black_box;
use std::time::Instant;

#[cfg(feature = "count-alloc")]
mod counting {
    use std::alloc::{GlobalAlloc, Layout, System};
    use std::sync::atomic::{AtomicU64, Ordering::Relaxed};
    pub static CALLS: AtomicU64 = AtomicU64::new(0);
    pub static REALLOCS: AtomicU64 = AtomicU64::new(0);
    pub static BYTES: AtomicU64 = AtomicU64::new(0);
    pub struct Counting;
    unsafe impl GlobalAlloc for Counting {
        unsafe fn alloc(&self, l: Layout) -> *mut u8 {
            CALLS.fetch_add(1, Relaxed);
            BYTES.fetch_add(l.size() as u64, Relaxed);
            unsafe { System.alloc(l) }
        }
        unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
            unsafe { System.dealloc(p, l) }
        }
        unsafe fn realloc(&self, p: *mut u8, l: Layout, n: usize) -> *mut u8 {
            REALLOCS.fetch_add(1, Relaxed);
            BYTES.fetch_add(n as u64, Relaxed);
            unsafe { System.realloc(p, l, n) }
        }
    }
    #[global_allocator]
    static A: Counting = Counting;
    pub fn snap() -> (u64, u64, u64) {
        (
            CALLS.load(Relaxed),
            REALLOCS.load(Relaxed),
            BYTES.load(Relaxed),
        )
    }
}

type Stage = (&'static str, fn(&mut State));
const STAGES: [Stage; 5] = [
    ("parse", stage_parse),
    ("rebuild", stage_rebuild),
    ("extract", stage_extract),
    ("encode", stage_encode),
    ("drop", stage_drop),
];

fn median(v: &mut [f64]) -> f64 {
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    v[v.len() / 2]
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let input = std::fs::read_to_string(&args[1]).expect("input file");
    let opt = |k: &str, d: &str| -> String {
        args.iter()
            .position(|a| a == k)
            .map(|i| args[i + 1].clone())
            .unwrap_or_else(|| d.to_string())
    };
    let iters: usize = opt("--iters", "30").parse().unwrap();
    let warmup: usize = opt("--warmup", "5").parse().unwrap();

    if args.iter().any(|a| a == "--loop") {
        let secs: f64 = opt("--seconds", "10").parse().unwrap();
        let t0 = Instant::now();
        let mut n = 0;
        while t0.elapsed().as_secs_f64() < secs {
            let mut s = State {
                input: input.clone(),
                ..State::default()
            };
            for (_, f) in STAGES {
                f(&mut s);
            }
            black_box(&s);
            n += 1;
        }
        eprintln!("loop: {n} calls");
        return;
    }

    let mut times: Vec<Vec<f64>> = vec![Vec::new(); STAGES.len()];
    let mut totals = Vec::new();
    #[cfg(feature = "count-alloc")]
    let mut allocs: Vec<(u64, u64, u64)> = vec![(0, 0, 0); STAGES.len()];
    for i in 0..warmup + iters {
        let mut s = State {
            input: input.clone(),
            ..State::default()
        };
        let mut total = 0.0;
        for (k, (_, f)) in STAGES.iter().enumerate() {
            #[cfg(feature = "count-alloc")]
            let a0 = counting::snap();
            let t = Instant::now();
            f(&mut s);
            let us = t.elapsed().as_secs_f64() * 1e6;
            #[cfg(feature = "count-alloc")]
            {
                let a1 = counting::snap();
                if i == warmup {
                    allocs[k] = (a1.0 - a0.0, a1.1 - a0.1, a1.2 - a0.2);
                }
            }
            if i >= warmup {
                times[k].push(us);
            }
            total += us;
        }
        if i >= warmup {
            totals.push(total);
        }
    }
    let mut out = serde_json::Map::new();
    for (k, (name, _)) in STAGES.iter().enumerate() {
        let min = times[k].iter().cloned().fold(f64::INFINITY, f64::min);
        #[allow(unused_mut)]
        let mut o = serde_json::json!({ "medianUs": median(&mut times[k]), "minUs": min });
        #[cfg(feature = "count-alloc")]
        {
            o["allocs"] = allocs[k].0.into();
            o["reallocs"] = allocs[k].1.into();
            o["bytes"] = allocs[k].2.into();
        }
        out.insert(name.to_string(), o);
    }
    out.insert(
        "total".into(),
        serde_json::json!({ "medianUs": median(&mut totals) }),
    );

    // Microbenchmarks: ns per unit, median of 15.
    let micro = |f: &dyn Fn() -> usize, units: f64| {
        let mut v: Vec<f64> = (0..15)
            .map(|_| {
                let t = Instant::now();
                black_box(f());
                t.elapsed().as_secs_f64() * 1e9 / units
            })
            .collect();
        median(&mut v)
    };
    out.insert(
        "microAllocNsPerRound".into(),
        micro(&|| micro_alloc(100_000), 100_000.0).into(),
    );
    out.insert(
        "microSipHashNsPerKey".into(),
        micro(&|| micro_siphash(1_000_000) as usize, 1_000_000.0).into(),
    );
    out.insert(
        "microMemcpyNsPer4KiB".into(),
        micro(&|| micro_memcpy(100_000), 100_000.0).into(),
    );
    println!("{}", serde_json::Value::Object(out));
}
