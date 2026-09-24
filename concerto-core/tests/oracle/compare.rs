//! Judges one fixture: compares what `ops::exec` produced against the
//! fixture's recorded `outcome`, as `migration/oracle/lib/judge.js` does
//! (README "Verdicts": `pass`, `fail`, `harness-error`). This harness adds
//! `unsupported` as its own bucket, distinct from `fail`, so "not ported
//! yet" is never counted as a Rust behavioural divergence, nor as a pass.

use serde_json::Value;

use super::fixture::Fixture;
use super::ledger::UNOWNED;
use super::ops::Dispatch;
use super::recipe::{Blocker, Fault};

#[derive(Debug)]
pub enum Verdict {
    /// The outcomes are identical: `ok` value and `effects`, or the error's
    /// class, message, location and component (PORTING.md section 2: "the
    /// same verdict, message, class and location").
    Pass,
    /// A real behavioural mismatch, or a state divergence while the inputs
    /// were rebuilt on the Rust engine. `kind` is what differs, the part the
    /// baseline records (`report.rs`); `detail` is the full first difference.
    Fail { kind: FailKind, detail: String },
    /// The op, or something these inputs need, has no Rust counterpart yet.
    /// `reason` says what; `blocker` names what blocks it when that is not
    /// the op itself, for its owner (`ledger.rs`).
    Unsupported {
        reason: String,
        blocker: Option<Blocker>,
    },
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
        // PORTING.md names no task that reproduces the seeded PRNG.
        return Verdict::Unsupported {
            reason: "env.random: the JS seeded PRNG is not reproduced".into(),
            blocker: Some(Blocker::Owner(UNOWNED.into())),
        };
    }

    let actual = match dispatch {
        Dispatch::Fault(Fault::Unsupported(reason)) => {
            return Verdict::Unsupported {
                reason,
                blocker: None,
            };
        }
        Dispatch::Fault(Fault::Blocked(reason, blocker)) => {
            return Verdict::Unsupported {
                reason,
                blocker: Some(blocker),
            };
        }
        Dispatch::Fault(Fault::Harness(detail)) => return Verdict::HarnessError { detail },
        Dispatch::Fault(Fault::Divergence(detail)) => {
            let kind = if detail.starts_with("input construction failed") {
                FailKind::InputConstruction
            } else {
                FailKind::StateDivergence
            };
            return Verdict::Fail { kind, detail };
        }
        Dispatch::Ran(outcome) => outcome,
    };

    match first_diff(&fixture.outcome.0, &actual, "$") {
        None => Verdict::Pass,
        Some(detail) => Verdict::Fail {
            kind: FailKind::of_diff(&detail),
            detail,
        },
    }
}

/// What a failing fixture got wrong: the coarse category of its first
/// difference, stable across runs and across message rewordings, which the
/// baseline stores per fixture so that a known failure that starts failing
/// for another reason is caught (`report.rs`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum FailKind {
    /// A recipe step replayed with another status or error class, or a
    /// handle it should have produced is missing.
    StateDivergence,
    /// Rebuilding an input (`new ModelFile` for an `mfnew`) failed.
    InputConstruction,
    /// TS returned a value, Rust threw.
    UnexpectedError,
    /// TS threw, Rust returned a value.
    MissingError,
    /// Both threw, with different classes.
    ClassMismatch,
    /// Both threw, with different components.
    ComponentMismatch,
    /// Both threw, at different locations.
    LocationMismatch,
    /// Both threw, with different messages.
    MessageMismatch,
    /// Both returned, with different values.
    ValueMismatch,
    /// The `effects` differ.
    EffectsMismatch,
}

impl FailKind {
    pub const ALL: [Self; 10] = [
        Self::StateDivergence,
        Self::InputConstruction,
        Self::UnexpectedError,
        Self::MissingError,
        Self::ClassMismatch,
        Self::ComponentMismatch,
        Self::LocationMismatch,
        Self::MessageMismatch,
        Self::ValueMismatch,
        Self::EffectsMismatch,
    ];

    /// Classifies a [`first_diff`] path. Keys are compared in sorted order,
    /// so for an error `class` wins over `component`, `location` and
    /// `message`: one fixture always gets the same kind.
    fn of_diff(detail: &str) -> Self {
        let path = detail.split(':').next().unwrap_or(detail);
        if path.starts_with("$.error.class") {
            Self::ClassMismatch
        } else if path.starts_with("$.error.component") {
            Self::ComponentMismatch
        } else if path.starts_with("$.error.location") {
            Self::LocationMismatch
        } else if path.starts_with("$.error.message") {
            Self::MessageMismatch
        } else if path.starts_with("$.error") {
            if detail.contains("expected (absent)") {
                Self::UnexpectedError
            } else {
                Self::MissingError
            }
        } else if path.starts_with("$.effects") {
            Self::EffectsMismatch
        } else {
            Self::ValueMismatch
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::StateDivergence => "state-divergence",
            Self::InputConstruction => "input-construction",
            Self::UnexpectedError => "unexpected-error",
            Self::MissingError => "missing-error",
            Self::ClassMismatch => "class-mismatch",
            Self::ComponentMismatch => "component-mismatch",
            Self::LocationMismatch => "location-mismatch",
            Self::MessageMismatch => "message-mismatch",
            Self::ValueMismatch => "value-mismatch",
            Self::EffectsMismatch => "effects-mismatch",
        }
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
