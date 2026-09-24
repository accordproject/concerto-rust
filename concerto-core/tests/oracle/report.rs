//! Aggregates verdicts per rule (op) and per fixture, and writes the report
//! the task's exit condition asks for: "failures are reported per rule (not
//! just a single pass/fail count), so the status reporter (P0-06) and the
//! dashboard (P0-08) can show which specific behaviours are still failing".

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use serde::Serialize;

use super::compare::Verdict;
use super::fixture::{Fixture, LoadError};

#[derive(Default)]
struct RuleCounts {
    pass: u64,
    fail: u64,
    unsupported: u64,
}

#[derive(Serialize)]
struct FixtureFailure {
    id: String,
    source: String,
    path: String,
    detail: String,
}

#[derive(Serialize)]
struct RuleReport {
    op: String,
    pass: u64,
    fail: u64,
    unsupported: u64,
}

#[derive(Serialize)]
pub struct Report {
    fixtures_dir: String,
    total_fixtures: u64,
    load_errors: u64,
    pass: u64,
    fail: u64,
    unsupported: u64,
    rules: Vec<RuleReport>,
    failures: Vec<FixtureFailure>,
    load_error_detail: Vec<String>,
}

pub struct Recorder {
    fixtures_dir: PathBuf,
    per_rule: BTreeMap<String, RuleCounts>,
    failures: Vec<FixtureFailure>,
    load_errors: Vec<LoadError>,
}

impl Recorder {
    pub fn new(fixtures_dir: PathBuf, load_errors: Vec<LoadError>) -> Self {
        Self {
            fixtures_dir,
            per_rule: BTreeMap::new(),
            failures: Vec::new(),
            load_errors,
        }
    }

    pub fn record(&mut self, fixture: &Fixture, verdict: Verdict) {
        let counts = self.per_rule.entry(fixture.op.clone()).or_default();
        match verdict {
            Verdict::Pass => counts.pass += 1,
            Verdict::Unsupported { .. } => counts.unsupported += 1,
            Verdict::Fail { detail } => {
                counts.fail += 1;
                self.failures.push(FixtureFailure {
                    id: fixture.id.clone(),
                    source: fixture.source.clone(),
                    path: fixture.path.display().to_string(),
                    detail,
                });
            }
        }
    }

    pub fn finish(self) -> Report {
        let mut rules: Vec<RuleReport> = self
            .per_rule
            .into_iter()
            .map(|(op, counts)| RuleReport {
                op,
                pass: counts.pass,
                fail: counts.fail,
                unsupported: counts.unsupported,
            })
            .collect();
        rules.sort_by(|a, b| a.op.cmp(&b.op));

        let pass = rules.iter().map(|r| r.pass).sum();
        let fail = rules.iter().map(|r| r.fail).sum();
        let unsupported = rules.iter().map(|r| r.unsupported).sum();

        Report {
            fixtures_dir: self.fixtures_dir.display().to_string(),
            total_fixtures: pass + fail + unsupported,
            load_errors: self.load_errors.len() as u64,
            pass,
            fail,
            unsupported,
            rules,
            failures: self.failures,
            load_error_detail: self
                .load_errors
                .iter()
                .map(|e| format!("{}: {}", e.path.display(), e.message))
                .collect(),
        }
    }
}

impl Report {
    /// Writes the machine-readable report (task's status reporter / P0-08
    /// dashboard) next to the workspace's `target/` directory, and prints a
    /// short per-rule summary to stdout.
    pub fn write(&self, path: &Path) {
        if let Some(parent) = path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        match serde_json::to_string_pretty(self) {
            Ok(json) => {
                if let Err(e) = fs::write(path, json) {
                    eprintln!(
                        "oracle harness: could not write report to {}: {e}",
                        path.display()
                    );
                }
            }
            Err(e) => eprintln!("oracle harness: could not serialise report: {e}"),
        }

        println!(
            "oracle harness: {} fixtures under {} ({} load errors) — {} pass, {} fail, {} unsupported",
            self.total_fixtures,
            self.fixtures_dir,
            self.load_errors,
            self.pass,
            self.fail,
            self.unsupported
        );
        println!(
            "oracle harness: per rule (ops this harness executes; every other op is 'unsupported'):"
        );
        for rule in &self.rules {
            if rule.pass + rule.fail > 0 {
                println!(
                    "  {:<55} pass={:<6} fail={:<6} unsupported={}",
                    rule.op, rule.pass, rule.fail, rule.unsupported
                );
            }
        }
        if !self.failures.is_empty() {
            println!("oracle harness: failing fixtures:");
            for failure in &self.failures {
                println!(
                    "  [{}] {} ({}): {}",
                    failure.source, failure.id, failure.path, failure.detail
                );
            }
        }
        println!("oracle harness: full report written to {}", path.display());
    }

    /// Fails the test when a fixture for an op this harness actually
    /// executes did not match the oracle: a real behavioural divergence,
    /// which is what "matching oracle fixtures pass" (plan §4, the exit
    /// condition every Phase 2 introspection task is judged against) means.
    /// `unsupported` fixtures never fail the test — they are exactly the
    /// ops a later task still has to add.
    pub fn assert_no_regressions(&self) {
        assert!(
            self.fail == 0,
            "{} oracle fixture(s) diverged from the reference for an op this harness runs; see the failures above and the report at the path printed above",
            self.fail
        );
    }
}
