//! P5-80 (accordproject/concerto-rust#424): the instance ops of the P5-15
//! crate sweep (`benches/p515_sweep.rs`), as a plain loop for callgrind.
//! Analysis only, not merged.
//!
//! ```text
//! cargo build --release --manifest-path benches/Cargo.toml --example p580_profile
//! valgrind --tool=callgrind --toggle-collect='p580_profile::op_*' \
//!     target/release/examples/p580_profile <op> <set> <iterations>
//! p580_profile time <op> <set> <iterations>   # wall clock, no valgrind
//! p580_profile cold <op> <set>                 # first pass on a fresh manager
//! p580_profile mem <set>                       # plan memory (live heap)
//! ```
//!
//! Each op runs over the same inputs as the crate sweep. The setup (model
//! load, the `from_json` that builds the instances) is outside the
//! `op_*` functions, so `--toggle-collect` counts only the op itself.
//! `CONCERTO_VALIDATION_PLAN=0|1` sets the prototype's plan switch
//! (`concerto_core::instance::plan::set_enabled`; on by default).

#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic, missing_docs)]

#[allow(dead_code)]
#[path = "../benches/common/mod.rs"]
mod common;

use std::alloc::{GlobalAlloc, Layout, System};
use std::hint::black_box;
use std::sync::atomic::{AtomicIsize, Ordering};
use std::time::Instant;

use concerto_core::instance::InstanceEnv;
use concerto_core::{ModelFile, ModelManager};
use concerto_core_js::value::Instance;
use concerto_core_js::{JsValue, Serializer, factory, resource};
use serde_json::Value;

/// Counts live heap bytes, for the plan's memory (`mem`).
struct Counting;

static LIVE: AtomicIsize = AtomicIsize::new(0);

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        LIVE.fetch_add(layout.size() as isize, Ordering::Relaxed);
        unsafe { System.alloc(layout) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        LIVE.fetch_sub(layout.size() as isize, Ordering::Relaxed);
        unsafe { System.dealloc(ptr, layout) }
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        LIVE.fetch_add(new_size as isize - layout.size() as isize, Ordering::Relaxed);
        unsafe { System.realloc(ptr, layout, new_size) }
    }
}

#[global_allocator]
static GLOBAL: Counting = Counting;

struct FixedEnv;

impl InstanceEnv for FixedEnv {
    fn new_id(&mut self) -> String {
        "00000000-0000-4000-8000-000000000000".into()
    }
    fn now_ms(&mut self) -> f64 {
        0.0
    }
}

fn without_enum_is_abstract(ast: &mut Value) {
    if let Some(decls) = ast.get_mut("declarations").and_then(Value::as_array_mut) {
        for d in decls {
            if d["$class"].as_str().is_some_and(|c| c.ends_with(".EnumDeclaration"))
                && let Some(o) = d.as_object_mut()
            {
                o.remove("isAbstract");
            }
        }
    }
}

struct Data {
    mm: ModelManager,
    instances: Vec<(String, Option<String>, Value)>,
}

fn load(set: &str) -> Data {
    let path = common::fixtures_dir()
        .join("p515")
        .join(format!("{set}.json"));
    let v: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    let mut mm = ModelManager::new().unwrap();
    for m in v["models"].as_array().unwrap() {
        let mut ast = m["ast"].clone();
        without_enum_is_abstract(&mut ast);
        let text = serde_json::to_string(&ast).unwrap();
        let mf = ModelFile::from_json_text(&text, None, Some(m["name"].as_str().unwrap().into()))
            .unwrap()
            .unwrap();
        mm.validate_and_add_model_file(mf).map_err(|(e, _)| e).unwrap();
    }
    let instances = v["instances"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| {
            (
                i["fqn"].as_str().unwrap().to_string(),
                i["id"].as_str().map(str::to_string),
                i["json"].clone(),
            )
        })
        .collect();
    Data { mm, instances }
}

#[inline(never)]
fn op_from_json(mm: &ModelManager, s: &Serializer, objects: &[JsValue]) {
    for o in objects {
        black_box(s.from_json(mm, o, None, &mut FixedEnv).unwrap());
    }
}

#[inline(never)]
fn op_to_json(mm: &ModelManager, s: &Serializer, resources: &[JsValue]) {
    for r in resources {
        black_box(s.to_json(mm, r, None).unwrap());
    }
}

#[inline(never)]
fn op_new_resource(mm: &ModelManager, targets: &[(String, String, JsValue)]) {
    for (ns, name, id) in targets {
        black_box(factory::new_resource(mm, ns, name, id.clone(), false, &mut FixedEnv).unwrap());
    }
}

#[inline(never)]
fn op_validate(mm: &ModelManager, insts: &mut [Instance]) {
    for inst in insts {
        resource::validate(mm, inst).unwrap();
    }
}

#[inline(never)]
fn op_set_property_value(mm: &ModelManager, insts: &mut [Instance], items: &[(usize, String, JsValue)]) {
    for (i, k, v) in items {
        resource::set_property_value(mm, &mut insts[*i], k, v.clone()).unwrap();
    }
}

#[inline(never)]
fn op_add_array_value(mm: &ModelManager, insts: &mut [Instance], items: &[(usize, String, JsValue)]) {
    for (i, k, v) in items {
        resource::add_array_value(mm, &mut insts[*i], k, v.clone()).unwrap();
    }
}

fn keys(d: &Data, insts: &[Instance], arrays: bool) -> Vec<(usize, String, JsValue)> {
    d.instances
        .iter()
        .enumerate()
        .flat_map(|(i, (_, _, j))| {
            j.as_object()
                .unwrap()
                .iter()
                .filter(|(k, v)| {
                    !k.starts_with('$')
                        && if arrays {
                            v.as_array().is_some_and(|a| !a.is_empty())
                        } else {
                            !v.is_array()
                        }
                })
                .map(move |(k, _)| (i, k.clone()))
        })
        .map(|(i, k)| {
            let v = match (arrays, insts[i].get(&k)) {
                (true, JsValue::Array(a)) => a[0].clone(),
                (true, _) => panic!("{k} is not an array"),
                (false, v) => v.clone(),
            };
            (i, k, v)
        })
        .collect()
}

fn apply_switch() {
    if let Ok(v) = std::env::var("CONCERTO_VALIDATION_PLAN") {
        concerto_core::instance::plan::set_enabled(v != "0");
    }
}

struct Inputs {
    objects: Vec<JsValue>,
    instances: Vec<Instance>,
    resources: Vec<JsValue>,
    targets: Vec<(String, String, JsValue)>,
    set_items: Vec<(usize, String, JsValue)>,
    add_items: Vec<(usize, String, JsValue)>,
}

fn inputs(d: &Data, s: &Serializer) -> Inputs {
    let mm = &d.mm;
    let objects: Vec<JsValue> = d.instances.iter().map(|(_, _, j)| JsValue::from_json(j)).collect();
    let instances: Vec<Instance> = objects
        .iter()
        .map(|o| s.from_json(mm, o, None, &mut FixedEnv).unwrap())
        .collect();
    let resources = instances
        .iter()
        .map(|i| JsValue::Instance(Box::new(i.clone())))
        .collect();
    let targets = d
        .instances
        .iter()
        .map(|(fqn, id, _)| {
            let decl = mm.declaration(mm.get_type_declaration(fqn).unwrap()).unwrap();
            let ns = fqn.rsplit_once('.').unwrap().0.to_string();
            (ns, decl.name().to_string(), id.clone().map_or(JsValue::Undefined, JsValue::String))
        })
        .collect();
    let set_items = keys(d, &instances, false);
    let add_items = keys(d, &instances, true);
    Inputs {
        objects,
        instances,
        resources,
        targets,
        set_items,
        add_items,
    }
}

/// One pass of `op` over the inputs; the number of items.
fn pass(op: &str, mm: &ModelManager, s: &Serializer, i: &Inputs, insts: &mut [Instance]) -> usize {
    match op {
        "from_json" => {
            op_from_json(mm, s, &i.objects);
            i.objects.len()
        }
        "to_json" => {
            op_to_json(mm, s, &i.resources);
            i.resources.len()
        }
        "new_resource" => {
            op_new_resource(mm, &i.targets);
            i.targets.len()
        }
        "validate" => {
            op_validate(mm, insts);
            insts.len()
        }
        "set_property_value" => {
            op_set_property_value(mm, insts, &i.set_items);
            i.set_items.len()
        }
        "add_array_value" => {
            op_add_array_value(mm, insts, &i.add_items);
            i.add_items.len()
        }
        other => panic!("unknown op {other}"),
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let mode = match args.get(1).map(String::as_str) {
        Some(m @ ("time" | "cold" | "mem")) => m,
        _ => "profile",
    };
    let rest = if mode == "profile" { &args[1..] } else { &args[2..] };
    apply_switch();
    let s = Serializer::new(true, true, None).unwrap();

    if mode == "mem" {
        // The live heap a set's instance passes leave behind in the
        // manager's caches (plans, inheritance, field defaults), on a fresh
        // manager: the plan's memory is this with the plan on minus off.
        let set = rest[0].as_str();
        let d = load(set);
        let i = inputs(&d, &s);
        let loaded = LIVE.load(Ordering::Relaxed);
        let Data {
            mm: fresh,
            instances: fresh_instances,
        } = load(set);
        drop(fresh_instances);
        let model_bytes = LIVE.load(Ordering::Relaxed) - loaded;
        let mut insts = i.instances.clone();
        let before = LIVE.load(Ordering::Relaxed);
        for op in ["from_json", "to_json", "validate", "set_property_value", "add_array_value"] {
            if op == "add_array_value" {
                insts.clone_from(&i.instances);
            }
            pass(op, &fresh, &s, &i, &mut insts);
        }
        if set != "concerto-core-test-data" {
            pass("new_resource", &fresh, &s, &i, &mut insts);
        }
        insts.clone_from(&i.instances);
        let after = LIVE.load(Ordering::Relaxed);
        let (plans, props) = concerto_core::instance::plan::stats(&fresh);
        println!(
            "{set}\tplan={}\tmodel_bytes={model_bytes}\tcache_bytes={}\tplans={plans}\tplan_props={props}",
            concerto_core::instance::plan::enabled(),
            after - before
        );
        return;
    }

    let op = rest[0].as_str();
    let set = rest[1].as_str();
    let iters: usize = rest.get(2).map_or(1, |n| n.parse().unwrap());
    let d = load(set);
    let i = inputs(&d, &s);
    let mut insts = i.instances.clone();

    if mode == "cold" {
        // The first pass on a fresh manager (no plan, inheritance or
        // field-default cache yet), then a second pass on it: the first-call
        // cost of building the plans is the cold pass minus the warm one.
        let fresh = load(set);
        let t = Instant::now();
        let n = pass(op, &fresh.mm, &s, &i, &mut insts);
        let cold = t.elapsed();
        if op == "add_array_value" {
            insts.clone_from(&i.instances);
        }
        let t = Instant::now();
        pass(op, &fresh.mm, &s, &i, &mut insts);
        let warm = t.elapsed();
        println!(
            "{op}\t{set}\tplan={}\tn={n}\tcold_us={:.1}\twarm_us={:.1}",
            concerto_core::instance::plan::enabled(),
            cold.as_secs_f64() * 1e6,
            warm.as_secs_f64() * 1e6
        );
        return;
    }

    let mut el = std::time::Duration::ZERO;
    let mut n = 0;
    for _ in 0..iters {
        if op == "add_array_value" {
            // Fresh instances each round, as the criterion bench's
            // `iter_batched`, so arrays do not grow without bound; not timed.
            insts.clone_from(&i.instances);
        }
        let start = Instant::now();
        n += pass(op, &d.mm, &s, &i, &mut insts);
        el += start.elapsed();
    }
    if mode == "time" {
        println!(
            "{op}\t{set}\tplan={}\t{n}\t{:.1}",
            concerto_core::instance::plan::enabled(),
            el.as_nanos() as f64 / n.max(1) as f64
        );
    }
}
