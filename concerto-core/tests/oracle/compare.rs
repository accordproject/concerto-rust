//! Judges one fixture: compares what `ops::exec` produced against the
//! fixture's recorded `outcome`, as `migration/oracle/lib/judge.js` does
//! (README "Verdicts": `pass`, `fail`, `harness-error`). This harness adds
//! `unsupported` as its own bucket, distinct from `fail`, so "not ported
//! yet" is never counted as a Rust behavioural divergence, nor as a pass.

use serde_json::Value;

use super::fixture::Fixture;
use super::ops::Dispatch;
use super::recipe::Fault;

#[derive(Debug)]
pub enum Verdict {
    /// The outcomes are identical: `ok` value and `effects`, or the error's
    /// class, message, location and component (PORTING.md section 2: "the
    /// same verdict, message, class and location").
    Pass,
    /// A real behavioural mismatch, or a state divergence while the inputs
    /// were rebuilt on the Rust engine.
    Fail { detail: String },
    /// The op, or something these inputs need, has no Rust counterpart yet.
    /// `reason` says what, and which task owns it.
    Unsupported { reason: String },
    /// The fixture could not be set up for reasons of the fixture or the
    /// CTO cache (a dangling reference, a missing cache entry). Never a
    /// pass: it fails the run (README "Verdicts").
    HarnessError { detail: String },
}

pub fn judge(fixture: &Fixture, dispatch: Dispatch) -> Verdict {
    if fixture.env.random {
        // README: "An engine that cannot reproduce that PRNG should compare
        // such fixtures structurally". No op this harness runs draws from
        // Math.random yet, so none is compared at all.
        return Verdict::Unsupported {
            reason: "env.random: the JS seeded PRNG is not reproduced".into(),
        };
    }

    let actual = match dispatch {
        Dispatch::Fault(Fault::Unsupported(reason)) => return Verdict::Unsupported { reason },
        Dispatch::Fault(Fault::Harness(detail)) => return Verdict::HarnessError { detail },
        Dispatch::Fault(Fault::Divergence(detail)) => return Verdict::Fail { detail },
        Dispatch::Ran(outcome) => outcome,
    };

    match first_diff(&fixture.outcome.0, &actual, "$") {
        None => Verdict::Pass,
        Some(detail) => Verdict::Fail { detail },
    }
}

/// JS equality of two canonical JSON values: object keys in any order, and
/// numbers compared as the IEEE doubles JS holds (`1` and `1.0` are the same
/// JS number; `serde_json` keeps them apart).
fn same(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Number(x), Value::Number(y)) => x.as_f64() == y.as_f64(),
        (Value::Array(x), Value::Array(y)) => {
            x.len() == y.len() && x.iter().zip(y).all(|(p, q)| same(p, q))
        }
        (Value::Object(x), Value::Object(y)) => {
            x.len() == y.len() && x.iter().all(|(k, v)| y.get(k).is_some_and(|w| same(v, w)))
        }
        _ => a == b,
    }
}

/// The first place, in sorted key order, where `expected` and `actual`
/// differ, as `judge.js`'s `firstDiff` reports it; `None` when they are the
/// same.
fn first_diff(expected: &Value, actual: &Value, path: &str) -> Option<String> {
    if same(expected, actual) {
        return None;
    }
    match (expected, actual) {
        (Value::Object(x), Value::Object(y)) => {
            let mut keys: Vec<&String> = x.keys().chain(y.keys()).collect();
            keys.sort();
            keys.dedup();
            for key in keys {
                let (p, q) = (x.get(key), y.get(key));
                match (p, q) {
                    (Some(p), Some(q)) => {
                        if let Some(d) = first_diff(p, q, &format!("{path}.{key}")) {
                            return Some(d);
                        }
                    }
                    _ => {
                        return Some(format!(
                            "{path}.{key}: expected {} got {}",
                            show(p),
                            show(q)
                        ));
                    }
                }
            }
            None
        }
        (Value::Array(x), Value::Array(y)) if x.len() == y.len() => x
            .iter()
            .zip(y)
            .enumerate()
            .find_map(|(i, (p, q))| first_diff(p, q, &format!("{path}.{i}"))),
        _ => Some(format!(
            "{path}: expected {} got {}",
            show(Some(expected)),
            show(Some(actual))
        )),
    }
}

fn show(v: Option<&Value>) -> String {
    const MAX: usize = 200;
    let Some(v) = v else {
        return "(absent)".into();
    };
    let s = v.to_string();
    if s.len() <= MAX {
        s
    } else {
        let mut end = MAX;
        while !s.is_char_boundary(end) {
            end -= 1;
        }
        format!("{}…", &s[..end])
    }
}
