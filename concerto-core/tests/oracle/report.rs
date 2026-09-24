//! Aggregates verdicts per rule (op) and per fixture, and writes the report
//! the task's exit condition asks for: "failures are reported per rule (not
//! just a single pass/fail count), so the status reporter (P0-06) and the
//! dashboard (P0-08) can show which specific behaviours are still failing".
//!
//! # The known-failures baseline
//!
//! The Rust engine is mid-port, so a full run has real failures: every
//! difference between the pre-port `ModelManager`/introspection code and TS
//! that a model-manager fixture exposes. They are reported per rule and per
//! fixture, and recorded in `known-failures.tsv` next to this file (`<op>
//! TAB <fixture id>`, sorted). The test fails on a **regression**: a fixture
//! that fails and is not in the baseline (PORTING.md 6.2, "No regressions
//! ... against the P1-07 baseline"). A baseline entry whose fixture now
//! passes is listed as fixed, so the task that fixed it can drop it from the
//! file. `ORACLE_UPDATE_BASELINE=1` rewrites the file from a full,
//! unfiltered run.
//!
//! Harness errors and load errors are never baselined: each one fails the
//! run (README: "A harness error is never a pass").

use std::collections::{BTreeMap, BTreeSet};
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
    harness_error: u64,
    reasons: BTreeMap<String, u64>,
    fail_reasons: BTreeMap<String, u64>,
}

#[derive(Serialize)]
struct FixtureProblem {
    op: String,
    id: String,
    source: String,
    path: String,
    detail: String,
}

#[derive(Serialize)]
struct Counted {
    reason: String,
    count: u64,
}

#[derive(Serialize)]
struct RuleReport {
    op: String,
    pass: u64,
    fail: u64,
    unsupported: u64,
    harness_error: u64,
    /// Why this rule's fixtures are unsupported, most common first.
    unsupported_reasons: Vec<Counted>,
    /// The failures' details with fixture-specific text trimmed, most
    /// common first.
    fail_kinds: Vec<Counted>,
}

#[derive(Serialize)]
pub struct Report {
    fixtures_dir: String,
    filter: Option<String>,
    total_fixtures: u64,
    load_errors: u64,
    pass: u64,
    fail: u64,
    unsupported: u64,
    harness_error: u64,
    /// Failing fixtures not in `known-failures.tsv`.
    regressions: u64,
    /// `known-failures.tsv` entries whose fixture passes now.
    fixed: Vec<String>,
    rules: Vec<RuleReport>,
    failures: Vec<FixtureProblem>,
    harness_errors: Vec<FixtureProblem>,
    load_error_detail: Vec<String>,
    #[serde(skip)]
    regression_detail: Vec<String>,
    #[serde(skip)]
    failing: BTreeSet<(String, String)>,
}

pub struct Recorder {
    fixtures_dir: PathBuf,
    filter: Option<String>,
    per_rule: BTreeMap<String, RuleCounts>,
    failures: Vec<FixtureProblem>,
    harness_errors: Vec<FixtureProblem>,
    passing: BTreeSet<(String, String)>,
    load_errors: Vec<LoadError>,
}

/// A failure detail with the fixture-specific parts (quoted values, long
/// tails) cut, so that failures of one kind group together per rule.
fn kind_of(detail: &str) -> String {
    let mut head = detail
        .split(" expected ")
        .next()
        .unwrap_or(detail)
        .to_string();
    if let Some(i) = head
        .find(": ")
        .filter(|_| head.starts_with("state divergence"))
    {
        let rest = &head[i + 2..];
        let cut = rest.find(": ").unwrap_or(rest.len());
        head = format!("{}: {}", &head[..i], &rest[..cut]);
    }
    if head.starts_with("input construction failed") {
        head = head.split(": ").take(3).collect::<Vec<_>>().join(": ");
    }
    head.chars().take(160).collect()
}

fn problem(fixture: &Fixture, detail: String) -> FixtureProblem {
    FixtureProblem {
        op: fixture.op.clone(),
        id: fixture.id.clone(),
        source: fixture.source.clone(),
        path: fixture.path.display().to_string(),
        detail,
    }
}

fn top(map: BTreeMap<String, u64>) -> Vec<Counted> {
    let mut v: Vec<Counted> = map
        .into_iter()
        .map(|(reason, count)| Counted { reason, count })
        .collect();
    v.sort_by(|a, b| b.count.cmp(&a.count).then_with(|| a.reason.cmp(&b.reason)));
    v
}

impl Recorder {
    pub fn new(fixtures_dir: PathBuf, filter: Option<String>, load_errors: Vec<LoadError>) -> Self {
        Self {
            fixtures_dir,
            filter,
            per_rule: BTreeMap::new(),
            failures: Vec::new(),
            harness_errors: Vec::new(),
            passing: BTreeSet::new(),
            load_errors,
        }
    }

    pub fn record(&mut self, fixture: &Fixture, verdict: Verdict) {
        let counts = self.per_rule.entry(fixture.op.clone()).or_default();
        match verdict {
            Verdict::Pass => {
                counts.pass += 1;
                self.passing
                    .insert((fixture.op.clone(), fixture.id.clone()));
            }
            Verdict::Unsupported { reason } => {
                counts.unsupported += 1;
                *counts.reasons.entry(reason).or_default() += 1;
            }
            Verdict::Fail { detail } => {
                counts.fail += 1;
                *counts.fail_reasons.entry(kind_of(&detail)).or_default() += 1;
                self.failures.push(problem(fixture, detail));
            }
            Verdict::HarnessError { detail } => {
                counts.harness_error += 1;
                self.harness_errors.push(problem(fixture, detail));
            }
        }
    }

    /// Totals, and the comparison with `baseline` (`(op, id)` pairs).
    pub fn finish(self, baseline: &BTreeSet<(String, String)>) -> Report {
        let rules: Vec<RuleReport> = self
            .per_rule
            .into_iter()
            .map(|(op, counts)| RuleReport {
                op,
                pass: counts.pass,
                fail: counts.fail,
                unsupported: counts.unsupported,
                harness_error: counts.harness_error,
                unsupported_reasons: top(counts.reasons),
                fail_kinds: top(counts.fail_reasons),
            })
            .collect();

        let pass = rules.iter().map(|r| r.pass).sum();
        let fail = rules.iter().map(|r| r.fail).sum();
        let unsupported = rules.iter().map(|r| r.unsupported).sum();
        let harness_error = rules.iter().map(|r| r.harness_error).sum();

        let failing: BTreeSet<(String, String)> = self
            .failures
            .iter()
            .map(|f| (f.op.clone(), f.id.clone()))
            .collect();
        let regression_detail: Vec<String> = self
            .failures
            .iter()
            .filter(|f| !baseline.contains(&(f.op.clone(), f.id.clone())))
            .map(|f| {
                format!(
                    "[{}] {} {} ({}): {}",
                    f.source, f.op, f.id, f.path, f.detail
                )
            })
            .collect();
        let fixed = baseline
            .iter()
            .filter(|key| self.passing.contains(*key))
            .map(|(op, id)| format!("{op}\t{id}"))
            .collect();

        Report {
            fixtures_dir: self.fixtures_dir.display().to_string(),
            filter: self.filter,
            total_fixtures: pass + fail + unsupported + harness_error,
            load_errors: self.load_errors.len() as u64,
            pass,
            fail,
            unsupported,
            harness_error,
            regressions: regression_detail.len() as u64,
            fixed,
            rules,
            failures: self.failures,
            harness_errors: self.harness_errors,
            load_error_detail: self
                .load_errors
                .iter()
                .map(|e| format!("{}: {}", e.path.display(), e.message))
                .collect(),
            regression_detail,
            failing,
        }
    }
}

/// Reads `known-failures.tsv`: `<op>\t<id>` lines; `#` starts a comment.
pub fn read_baseline(path: &Path) -> BTreeSet<(String, String)> {
    let Ok(text) = fs::read_to_string(path) else {
        return BTreeSet::new();
    };
    text.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .filter_map(|l| {
            let (op, id) = l.split_once('\t')?;
            Some((op.to_string(), id.to_string()))
        })
        .collect()
}

impl Report {
    /// Writes the machine-readable report (the status reporter's and the
    /// P0-08 dashboard's input) and prints a per-rule summary.
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
            "oracle harness: {} fixtures under {}{} ({} load errors): {} pass, {} fail ({} not in \
             the baseline), {} unsupported, {} harness errors",
            self.total_fixtures,
            self.fixtures_dir,
            self.filter
                .as_ref()
                .map(|f| format!(" matching ORACLE_OP={f}"))
                .unwrap_or_default(),
            self.load_errors,
            self.pass,
            self.fail,
            self.regressions,
            self.unsupported,
            self.harness_error
        );
        println!("oracle harness: per rule (pass / fail / unsupported / harness error):");
        for rule in &self.rules {
            println!(
                "  {:<60} {:>6} {:>6} {:>6} {:>4}",
                rule.op, rule.pass, rule.fail, rule.unsupported, rule.harness_error
            );
            for kind in rule.fail_kinds.iter().take(3) {
                println!("      fail x{}: {}", kind.count, kind.reason);
            }
            if rule.pass + rule.fail + rule.harness_error == 0
                && let Some(reason) = rule.unsupported_reasons.first()
            {
                println!("      unsupported: {}", reason.reason);
            }
        }
        for problem in &self.harness_errors {
            println!(
                "ERROR harness error [{}] {} {} ({}): {}",
                problem.source, problem.op, problem.id, problem.path, problem.detail
            );
        }
        for line in &self.regression_detail {
            println!("ERROR regression (not in known-failures.tsv) {line}");
        }
        if !self.fixed.is_empty() {
            println!(
                "oracle harness: {} known failures now pass; drop them from known-failures.tsv:",
                self.fixed.len()
            );
            for key in &self.fixed {
                println!("  fixed {key}");
            }
        }
        println!("oracle harness: full report written to {}", path.display());
    }

    /// Rewrites the baseline from this run's failures.
    pub fn write_baseline(&self, path: &Path) {
        let mut text = String::from(
            "# Oracle fixtures the native harness knows to fail, one `<op>\\t<fixture id>` per line\n\
             # (concerto-core/tests/oracle/report.rs). A failing fixture not listed here fails\n\
             # `cargo test --test oracle`; regenerate with ORACLE_UPDATE_BASELINE=1 on a full run.\n",
        );
        for (op, id) in &self.failing {
            text.push_str(&format!("{op}\t{id}\n"));
        }
        fs::write(path, text).expect("write known-failures.tsv");
        println!(
            "oracle harness: wrote {} known failures to {}",
            self.failing.len(),
            path.display()
        );
    }

    /// Fails the test on a load error, a harness error, or a failure that
    /// is not in the baseline. `unsupported` fixtures never fail it.
    pub fn assert_no_regressions(&self) {
        assert!(
            self.load_errors == 0,
            "{} oracle fixture file(s) could not be loaded (malformed JSON, a missing or corrupt \
             blob, or a shape this harness's Fixture does not model); see load_error_detail in \
             the report at the path printed above. A harness error is never a pass",
            self.load_errors
        );
        assert!(
            self.harness_error == 0,
            "{} oracle fixture(s) could not be set up (see the ERROR harness error lines above). A \
             harness error is never a pass",
            self.harness_error
        );
        assert!(
            self.regressions == 0,
            "{} oracle fixture(s) failed that are not in known-failures.tsv (see the ERROR \
             regression lines above and the report at the path printed above)",
            self.regressions
        );
    }
}
