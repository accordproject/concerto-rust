//! P5-88 (accordproject/concerto-rust#434): the validation plan's parity
//! property over the oracle's instance fixtures. Every fixture that runs an
//! instance op, or decodes an instance among its inputs, is replayed twice
//! on the same thread: once as shipped (the plan on), once with the plan
//! turned off (`concerto_core::instance::plan::testing::without_plan`, the
//! unplanned path the plan falls back to). The two outcomes must be equal:
//! the same success value, or the same error (class, message and location),
//! so the plan changes no throw scenario, no exception class and not the
//! order in which the first error is found.

use std::fmt::Write as _;

use concerto_core::instance::plan::testing::without_plan;
use serde_json::Value;

use crate::Harness;
use crate::fixture::Fixture;
use crate::ops::{self, Dispatch};
use crate::{compare, instances};

/// The op classes of the instance paths the plan serves.
const INSTANCE_CLASSES: &[&str] = &[
    "Serializer",
    "Factory",
    "Resource",
    "Typed",
    "Identifiable",
    "Relationship",
    "Concept",
    "ResourceValidator",
];

/// Whether `fx` reaches an instance path: an instance op, or an op whose
/// inputs carry an instance (`{"@@oracle":"typed",…}`).
pub fn is_instance_fixture(fx: &Fixture) -> bool {
    let class = fx.op.split('.').next().unwrap_or_default();
    INSTANCE_CLASSES.contains(&class) || has_typed_input(fx)
}

fn has_typed_input(fx: &Fixture) -> bool {
    fn typed(v: &Value) -> bool {
        match v {
            Value::Object(o) => {
                o.get(crate::recipe::M).and_then(Value::as_str) == Some("typed")
                    || o.values().any(typed)
            }
            Value::Array(a) => a.iter().any(typed),
            _ => false,
        }
    }
    fx.inputs.target.as_ref().is_some_and(typed) || fx.inputs.args.iter().any(typed)
}

/// Runs `fx`'s op, and describes the dispatch as text that compares every
/// part of it. A generated UUID, and a clock reading taken while the op ran,
/// are masked as the judge masks them (`compare::canonicalise`), so two runs
/// compare equal when only the clock or the random identifier differ.
fn run(harness: &Harness, fx: &Fixture) -> String {
    let start = instances::now_ms();
    let dispatch = ops::exec(harness, &fx.op, &fx.inputs);
    let window = Some((start, instances::now_ms()));
    let canonical = |v: &Value| compare::canonicalise(v, &fx.inputs, window);
    match &dispatch {
        Dispatch::Ran(v) => format!("ran {}", canonical(v)),
        Dispatch::RanAttributed(v, a) => format!("ran {} attributed {a:?}", canonical(v)),
        Dispatch::Fault(f) => format!("fault {f:?}"),
    }
}

/// Replays every instance fixture with the plan on and off; returns how
/// many were compared, and a description of each that differed.
pub fn compare(harness: &Harness, fixtures: &[Fixture]) -> (usize, Vec<String>) {
    let mut compared = 0;
    let mut differences = Vec::new();
    for fx in fixtures.iter().filter(|fx| is_instance_fixture(fx)) {
        compared += 1;
        let planned = run(harness, fx);
        let unplanned = without_plan(|| run(harness, fx));
        if planned != unplanned {
            let mut d = String::new();
            let _ = write!(
                d,
                "{} {} ({}):\n  plan on:  {planned}\n  plan off: {unplanned}",
                fx.op,
                fx.id,
                fx.path.display()
            );
            differences.push(d);
        }
    }
    (compared, differences)
}
