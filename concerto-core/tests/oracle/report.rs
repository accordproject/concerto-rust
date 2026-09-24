//! Aggregates verdicts per rule (op) and per fixture, and writes the report
//! the task's exit condition asks for: "failures are reported per rule (not
//! just a single pass/fail count), so the status reporter (P0-06) and the
//! dashboard (P0-08) can show which specific behaviours are still failing".
//!
//! # The baseline
//!
//! The Rust engine is mid-port, so a full run has real failures: every
//! difference between the pre-port `ModelManager`/introspection code and TS
//! that a model-manager fixture exposes. `baseline.tsv` next to this file
//! records the verdict of every fixture that was compared on the last full
//! run, one `<op> TAB <fixture id> TAB <status>` line per fixture, sorted by
//! op and id, where `<status>` is `pass` or `fail:<kind>` (the
//! [`FailKind`] of its first difference: `message-mismatch`,
//! `class-mismatch`, `state-divergence`, ...). The test fails on a
//! **regression** (PORTING.md 6.2, "No regressions ... against the P1-07
//! baseline"):
//!
//! - a fixture fails that the baseline does not list as failing;
//! - a baselined failure fails with another kind (it started failing for a
//!   different reason);
//! - a baselined fixture (passing or failing) is now `unsupported` or a
//!   harness error: it dropped out of the comparison.
//!
//! A baselined failure that passes now is listed as fixed, a fixture that
//! passes without being baselined as new, and a baselined fixture that is
//! not in the corpus at all as missing. On a full, unfiltered run each of
//! these fails the run too, until `ORACLE_UPDATE_BASELINE=1` rewrites the
//! file and the delta is committed: otherwise a fixed fixture would stay
//! `fail:<kind>` in the baseline, and a later change could break it again
//! with the same kind unnoticed. With `ORACLE_OP` they are only reported,
//! and only baseline entries whose op matches are checked.
//!
//! The baseline stores the failure kind only, not a digest of the message:
//! the Rust side of a message mismatch is pre-port text that every porting
//! task rewrites on its way to the TS text, so a digest would flag progress
//! as a regression, while the kind still catches a fixture whose failure
//! changes character.
//!
//! Harness errors and load errors are never baselined: each one fails the
//! run (README: "A harness error is never a pass").

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use serde::Serialize;

use super::compare::{FailKind, Verdict};
use super::fixture::{Fixture, LoadError};
use super::ledger::{Ledger, UNOWNED};
use super::recipe::Blocker;

#[derive(Default)]
struct RuleCounts {
    pass: u64,
    fail: u64,
    unsupported: u64,
    harness_error: u64,
    /// `(reason, owner)` to count.
    reasons: BTreeMap<(String, String), u64>,
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
    #[serde(skip_serializing_if = "Option::is_none")]
    owner: Option<String>,
    count: u64,
}

#[derive(Serialize)]
struct RuleReport {
    op: String,
    /// The task that owns this op (`ledger.rs`): the owner of its failures,
    /// and of its unsupported fixtures unless a reason names another.
    owner: String,
    pass: u64,
    fail: u64,
    unsupported: u64,
    harness_error: u64,
    /// Why this rule's fixtures are unsupported, with the owner of what
    /// blocks them, most common first.
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
    /// Unsupported or failing fixtures whose owner is `unowned`.
    unowned: u64,
    /// Unsupported and failing fixtures per owner.
    owners: BTreeMap<String, u64>,
    /// Fixtures whose verdict regressed against `baseline.tsv`.
    regressions: u64,
    /// Baselined failures that pass now.
    fixed: Vec<String>,
    /// Passing fixtures `baseline.tsv` does not list.
    new_passes: Vec<String>,
    /// Baselined fixtures absent from this corpus.
    missing: Vec<String>,
    rules: Vec<RuleReport>,
    failures: Vec<FixtureProblem>,
    harness_errors: Vec<FixtureProblem>,
    load_error_detail: Vec<String>,
    #[serde(skip)]
    regression_detail: Vec<String>,
    #[serde(skip)]
    statuses: BTreeMap<(String, String), Status>,
}

/// One fixture's verdict, as the baseline records it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Pass,
    Fail(FailKind),
    Unsupported,
    HarnessError,
}

impl Status {
    fn as_string(self) -> String {
        match self {
            Self::Pass => "pass".into(),
            Self::Fail(kind) => format!("fail:{}", kind.as_str()),
            Self::Unsupported => "unsupported".into(),
            Self::HarnessError => "harness-error".into(),
        }
    }

    fn parse(text: &str) -> Option<Self> {
        if text == "pass" {
            return Some(Self::Pass);
        }
        let kind = text.strip_prefix("fail:")?;
        FailKind::ALL
            .into_iter()
            .find(|k| k.as_str() == kind)
            .map(Self::Fail)
    }
}

/// The baseline: `(op, id)` to the recorded status (`pass` or `fail:<kind>`).
pub type Baseline = BTreeMap<(String, String), Status>;

pub struct Recorder {
    fixtures_dir: PathBuf,
    filter: Option<String>,
    per_rule: BTreeMap<String, RuleCounts>,
    rule_owner: BTreeMap<String, String>,
    owners: BTreeMap<String, u64>,
    failures: Vec<FixtureProblem>,
    harness_errors: Vec<FixtureProblem>,
    statuses: BTreeMap<(String, String), Status>,
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

fn top<K>(map: BTreeMap<K, u64>, split: impl Fn(K) -> (String, Option<String>)) -> Vec<Counted> {
    let mut v: Vec<Counted> = map
        .into_iter()
        .map(|(key, count)| {
            let (reason, owner) = split(key);
            Counted {
                reason,
                owner,
                count,
            }
        })
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
            rule_owner: BTreeMap::new(),
            owners: BTreeMap::new(),
            failures: Vec::new(),
            harness_errors: Vec::new(),
            statuses: BTreeMap::new(),
            load_errors,
        }
    }

    /// Records one verdict. `owners` resolves an op or blocking member to
    /// its owning task (`ledger.rs`).
    pub fn record(&mut self, fixture: &Fixture, verdict: Verdict, owners: &Ledger) {
        let op_owner = self
            .rule_owner
            .entry(fixture.op.clone())
            .or_insert_with(|| owners.owner(&fixture.op))
            .clone();
        let owner = match &verdict {
            Verdict::Pass | Verdict::HarnessError { .. } => None,
            Verdict::Fail { .. } | Verdict::Unsupported { blocker: None, .. } => Some(op_owner),
            Verdict::Unsupported {
                blocker: Some(Blocker::Member(member)),
                ..
            } => Some(owners.owner(member)),
            Verdict::Unsupported {
                blocker: Some(Blocker::Owner(owner)),
                ..
            } => Some(owner.clone()),
        };
        if let Some(owner) = &owner {
            *self.owners.entry(owner.clone()).or_default() += 1;
        }
        let counts = self.per_rule.entry(fixture.op.clone()).or_default();
        let status = match verdict {
            Verdict::Pass => {
                counts.pass += 1;
                Status::Pass
            }
            Verdict::Unsupported { reason, .. } => {
                counts.unsupported += 1;
                *counts
                    .reasons
                    .entry((reason, owner.unwrap_or_default()))
                    .or_default() += 1;
                Status::Unsupported
            }
            Verdict::Fail { kind, detail } => {
                counts.fail += 1;
                *counts.fail_reasons.entry(kind_of(&detail)).or_default() += 1;
                self.failures.push(problem(fixture, detail));
                Status::Fail(kind)
            }
            Verdict::HarnessError { detail } => {
                counts.harness_error += 1;
                self.harness_errors.push(problem(fixture, detail));
                Status::HarnessError
            }
        };
        let previous = self
            .statuses
            .insert((fixture.op.clone(), fixture.id.clone()), status);
        assert!(
            previous.is_none(),
            "oracle fixture {} {} ({}) appears twice in the corpus",
            fixture.op,
            fixture.id,
            fixture.path.display()
        );
    }

    /// Totals, and the comparison with `baseline` (`(op, id)` pairs).
    pub fn finish(self, baseline: &Baseline) -> Report {
        let rules: Vec<RuleReport> = self
            .per_rule
            .into_iter()
            .map(|(op, counts)| RuleReport {
                owner: self.rule_owner.get(&op).cloned().unwrap_or_default(),
                op,
                pass: counts.pass,
                fail: counts.fail,
                unsupported: counts.unsupported,
                harness_error: counts.harness_error,
                unsupported_reasons: top(counts.reasons, |(r, o)| (r, Some(o))),
                fail_kinds: top(counts.fail_reasons, |r| (r, None)),
            })
            .collect();

        let pass = rules.iter().map(|r| r.pass).sum();
        let fail = rules.iter().map(|r| r.fail).sum();
        let unsupported = rules.iter().map(|r| r.unsupported).sum();
        let harness_error = rules.iter().map(|r| r.harness_error).sum();

        let detail_of = |op: &str, id: &str| {
            self.failures
                .iter()
                .chain(&self.harness_errors)
                .find(|f| f.op == op && f.id == id)
                .map(|f| format!("[{}] ({}): {}", f.source, f.path, f.detail))
                .unwrap_or_default()
        };
        let in_scope = |op: &str| {
            self.filter
                .as_ref()
                .is_none_or(|f| op.starts_with(f.as_str()))
        };
        let mut regression_detail = Vec::new();
        let mut fixed = Vec::new();
        let mut new_passes = Vec::new();
        let mut missing = Vec::new();
        for ((op, id), was) in baseline {
            if !in_scope(op) {
                continue;
            }
            let key = format!("{op}\t{id}");
            match self.statuses.get(&(op.clone(), id.clone())) {
                None => missing.push(key),
                Some(now) if now == was => {}
                Some(Status::Pass) => fixed.push(key),
                Some(now) => regression_detail.push(format!(
                    "{op} {id}: baseline {}, now {} {}",
                    was.as_string(),
                    now.as_string(),
                    detail_of(op, id)
                )),
            }
        }
        for ((op, id), now) in &self.statuses {
            if baseline.contains_key(&(op.clone(), id.clone())) {
                continue;
            }
            match now {
                Status::Pass => new_passes.push(format!("{op}\t{id}")),
                Status::Fail(_) => regression_detail.push(format!(
                    "{op} {id}: not in the baseline, now {} {}",
                    now.as_string(),
                    detail_of(op, id)
                )),
                Status::Unsupported | Status::HarnessError => {}
            }
        }

        Report {
            fixtures_dir: self.fixtures_dir.display().to_string(),
            filter: self.filter,
            total_fixtures: pass + fail + unsupported + harness_error,
            load_errors: self.load_errors.len() as u64,
            pass,
            fail,
            unsupported,
            harness_error,
            unowned: self.owners.get(UNOWNED).copied().unwrap_or(0),
            owners: self.owners,
            regressions: regression_detail.len() as u64,
            fixed,
            new_passes,
            missing,
            rules,
            failures: self.failures,
            harness_errors: self.harness_errors,
            load_error_detail: self
                .load_errors
                .iter()
                .map(|e| format!("{}: {}", e.path.display(), e.message))
                .collect(),
            regression_detail,
            statuses: self.statuses,
        }
    }
}

/// Reads `baseline.tsv` (module doc); `#` starts a comment. A line this
/// harness cannot read panics: a corrupt baseline must not pass quietly.
pub fn read_baseline(path: &Path) -> Baseline {
    let Ok(text) = fs::read_to_string(path) else {
        return Baseline::new();
    };
    let mut baseline = Baseline::new();
    for l in text
        .lines()
        .filter(|l| !l.trim().is_empty() && !l.starts_with('#'))
    {
        let mut fields = l.split('\t');
        let (Some(op), Some(id), Some(status), None) =
            (fields.next(), fields.next(), fields.next(), fields.next())
        else {
            panic!("{}: malformed baseline line {l:?}", path.display());
        };
        let status = Status::parse(status)
            .unwrap_or_else(|| panic!("{}: unknown status in baseline line {l:?}", path.display()));
        let previous = baseline.insert((op.to_string(), id.to_string()), status);
        assert!(
            previous.is_none(),
            "{}: {op} {id} is listed twice",
            path.display()
        );
    }
    baseline
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
            "oracle harness: {} fixtures under {}{} ({} load errors): {} pass, {} fail, {} unsupported, {} harness errors; {} \
             regressions against baseline.tsv",
            self.total_fixtures,
            self.fixtures_dir,
            self.filter
                .as_ref()
                .map(|f| format!(" matching ORACLE_OP={f}"))
                .unwrap_or_default(),
            self.load_errors,
            self.pass,
            self.fail,
            self.unsupported,
            self.harness_error,
            self.regressions
        );
        println!(
            "oracle harness: owners of the unsupported and failing fixtures ({} unowned): {}",
            self.unowned,
            self.owners
                .iter()
                .map(|(owner, n)| format!("{owner} {n}"))
                .collect::<Vec<_>>()
                .join(", ")
        );
        println!("oracle harness: per rule (pass / fail / unsupported / harness error  [owner]):");
        for rule in &self.rules {
            println!(
                "  {:<60} {:>6} {:>6} {:>6} {:>4}  [{}]",
                rule.op, rule.pass, rule.fail, rule.unsupported, rule.harness_error, rule.owner
            );
            for kind in rule.fail_kinds.iter().take(3) {
                println!("      fail x{}: {}", kind.count, kind.reason);
            }
            if rule.pass + rule.fail + rule.harness_error == 0
                && let Some(reason) = rule.unsupported_reasons.first()
            {
                println!(
                    "      unsupported: {} [{}]",
                    reason.reason,
                    reason.owner.as_deref().unwrap_or_default()
                );
            }
        }
        for problem in &self.harness_errors {
            println!(
                "ERROR harness error [{}] {} {} ({}): {}",
                problem.source, problem.op, problem.id, problem.path, problem.detail
            );
        }
        for line in &self.regression_detail {
            println!("ERROR regression against baseline.tsv: {line}");
        }
        for (label, keys) in [
            ("baselined failures now pass", &self.fixed),
            (
                "passing fixtures are not in the baseline yet",
                &self.new_passes,
            ),
            ("baselined fixtures are not in this corpus", &self.missing),
        ] {
            if !keys.is_empty() {
                println!(
                    "oracle harness: {} {label} (ORACLE_UPDATE_BASELINE=1 takes them in):",
                    keys.len()
                );
                for key in keys.iter().take(20) {
                    println!("  {key}");
                }
            }
        }
        println!("oracle harness: full report written to {}", path.display());
    }

    /// Rewrites the baseline from this run: every compared fixture, sorted.
    /// Only a valid, full, unfiltered run may do so: a run with harness or
    /// load errors (a broken CTO cache, say) would silently shrink it.
    pub fn write_baseline(&self, path: &Path) {
        assert!(
            self.filter.is_none(),
            "ORACLE_UPDATE_BASELINE=1 needs a full run: unset ORACLE_OP"
        );
        self.assert_valid_run();
        let mut text = String::from(
            "# The native oracle harness's baseline (concerto-core/tests/oracle/report.rs): the\n\
             # verdict of every compared fixture, `<op>\\t<fixture id>\\t<pass | fail:<kind>>`.\n\
             # A fixture that regresses against it fails `cargo test --test oracle`; regenerate\n\
             # with ORACLE_UPDATE_BASELINE=1 on a full, unfiltered run.\n",
        );
        let (mut passes, mut fails) = (0, 0);
        for ((op, id), status) in &self.statuses {
            match status {
                Status::Pass => passes += 1,
                Status::Fail(_) => fails += 1,
                Status::Unsupported | Status::HarnessError => continue,
            }
            text.push_str(&format!("{op}\t{id}\t{}\n", status.as_string()));
        }
        fs::write(path, text).expect("write baseline.tsv");
        println!(
            "oracle harness: wrote {passes} passing and {fails} failing fixtures to {}",
            path.display()
        );
    }

    /// Fails the test when the run itself is invalid: a fixture file that
    /// did not load, or a fixture that could not be set up (a missing CTO
    /// cache entry, say). Nothing is judged, or written, from such a run.
    pub fn assert_valid_run(&self) {
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
    }

    /// Fails the test on an invalid run ([`Self::assert_valid_run`]), a
    /// regression against the baseline (module doc), or, on a full,
    /// unfiltered run, a baselined fixture that is not in the corpus.
    pub fn assert_no_regressions(&self) {
        self.assert_valid_run();
        assert!(
            self.regressions == 0,
            "{} oracle fixture(s) regressed against baseline.tsv (see the ERROR regression \
             lines above and the report at the path printed above)",
            self.regressions
        );
        assert!(
            self.filter.is_some() || (self.fixed.is_empty() && self.new_passes.is_empty()),
            "{} baselined failure(s) now pass and {} passing fixture(s) are not in the baseline \
             (listed above): record them, or a later change could break them again unnoticed. \
             Regenerate with ORACLE_UPDATE_BASELINE=1 on a full run and commit the baseline.tsv \
             delta",
            self.fixed.len(),
            self.new_passes.len()
        );
        assert!(
            self.filter.is_some() || self.missing.is_empty(),
            "{} baselined fixture(s) are not in the corpus (listed above): the corpus was \
             re-recorded or truncated; regenerate the baseline with ORACLE_UPDATE_BASELINE=1 if \
             that is intended",
            self.missing.len()
        );
    }
}
