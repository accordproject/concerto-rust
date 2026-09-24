//! Judges one fixture: compares what `ops::exec` produced against the
//! fixture's recorded `outcome` (README "Verdicts": `pass`, `fail`,
//! `harness-error` — this harness adds `unsupported` as its own bucket,
//! distinct from `fail`, so "not implemented yet" is never counted as a
//! Rust behavioural divergence).

use super::fixture::{Fixture, Outcome};
use super::ops::{Dispatch, ExecOutcome};

#[derive(Debug)]
pub enum Verdict {
    /// The canonical outcomes matched (README: `outcome is canonical`;
    /// `serde_json::Value` equality is already key-order independent, which
    /// is all the canonicalisation the ops this harness runs need — none of
    /// them mint a `<uuid>` or `<now>`).
    Pass,
    /// A real behavioural mismatch: this is what the task's exit condition
    /// means by "failures ... reported per rule".
    Fail { detail: String },
    /// The op, or these particular arguments, are not implemented by this
    /// harness yet (`ops.rs`'s module doc lists what is). Never asserted
    /// against — see `report.rs`. `reason` is diagnostic only (surfaced by
    /// `{:?}`, e.g. in the self-tests' failure messages).
    #[allow(dead_code)]
    Unsupported { reason: String },
}

pub fn judge(fixture: &Fixture, dispatch: Dispatch) -> Verdict {
    if fixture.env.random {
        // README: "An engine that cannot reproduce that PRNG should compare
        // such fixtures structurally". This harness does not run any op
        // that draws from Math.random yet (see ops.rs), so this is a no-op
        // guard for when one is added.
        return Verdict::Unsupported {
            reason: "env.random: JS seeded PRNG not reproduced".into(),
        };
    }

    let outcome = match dispatch {
        Dispatch::Unsupported(reason) => return Verdict::Unsupported { reason },
        Dispatch::Ran(outcome) => outcome,
    };

    match (&fixture.outcome, outcome) {
        (Outcome::Ok { ok: expected, .. }, ExecOutcome::Ok(actual)) => {
            if *expected == actual {
                Verdict::Pass
            } else {
                Verdict::Fail {
                    detail: format!(
                        "expected ok {} got ok {}",
                        truncate(&expected.to_string()),
                        truncate(&actual.to_string())
                    ),
                }
            }
        }
        (Outcome::Ok { ok, .. }, ExecOutcome::Err(actual)) => Verdict::Fail {
            detail: format!(
                "expected ok {} got error {}: {}",
                truncate(&ok.to_string()),
                actual.class,
                truncate(&actual.message)
            ),
        },
        (Outcome::Err { error }, ExecOutcome::Ok(actual)) => Verdict::Fail {
            detail: format!(
                "expected error {}: {} got ok {}",
                error.class,
                truncate(&error.message),
                truncate(&actual.to_string())
            ),
        },
        (Outcome::Err { error: expected }, ExecOutcome::Err(actual)) => {
            let mut mismatches = Vec::new();
            if expected.class != actual.class {
                mismatches.push(format!("class: {} != {}", expected.class, actual.class));
            }
            if expected.message != actual.message {
                mismatches.push(format!(
                    "message: {:?} != {:?}",
                    truncate(&expected.message),
                    truncate(&actual.message)
                ));
            }
            let expected_component = expected.component.as_deref();
            if expected_component != actual.component {
                mismatches.push(format!(
                    "component: {expected_component:?} != {:?}",
                    actual.component
                ));
            }
            if expected.location != actual.location {
                mismatches.push(format!(
                    "location: {:?} != {:?}",
                    expected.location, actual.location
                ));
            }
            if mismatches.is_empty() {
                Verdict::Pass
            } else {
                Verdict::Fail {
                    detail: mismatches.join("; "),
                }
            }
        }
    }
}

fn truncate(s: &str) -> String {
    const MAX: usize = 300;
    if s.len() <= MAX {
        s.to_string()
    } else {
        format!("{}…", &s[..MAX])
    }
}
