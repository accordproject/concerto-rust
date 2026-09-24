//! The native oracle harness (task P1-07, plan §2.5 and §4 phase 1;
//! `accordproject/concerto-rust#44`): a `cargo test` that reads every
//! fixture under `accordproject/concerto`'s `migration/oracle/fixtures/**`
//! (task P0-05's behavioural oracle) and replays each `{op, inputs,
//! outcome}` fixture directly against this crate, using the error contract
//! and message catalogue from task P1-05 (`concerto_core::error`) to
//! compare verdicts, messages, exception class and location.
//!
//! This is the first of the three places the oracle runs (plan §2.5): a
//! native harness, as opposed to the WASM adapter
//! (`migration/oracle/lib/rust-adapter.js`) or a future CI job. Every
//! Phase 2 introspection task is judged against it ("matching oracle
//! fixtures pass").
//!
//! # Locating the corpus
//!
//! The corpus (README: "kept out of the concerto PR on purpose", ~74 MB,
//! ~15.6k files) is generated, not committed, and lives in the sibling
//! `accordproject/concerto` checkout. This test looks for it, in order:
//!
//! 1. `$CONCERTO_ORACLE_FIXTURES`, if set (a `migration/oracle/fixtures`
//!    directory, or a directory containing one);
//! 2. `../concerto/migration/oracle/fixtures` next to this repository
//!    checkout (the layout `migration/oracle/README.md`'s own commands and
//!    this repo's `PORTING.md` assume: `concerto` and `concerto-rust`
//!    cloned as siblings);
//! 3. otherwise the test is skipped, with a message explaining how to
//!    generate the corpus (`migration/oracle/bin/record-all.sh` in the
//!    `concerto` checkout).
//!
//! # What is compared
//!
//! `ops.rs`'s module doc lists exactly which ops this harness executes today
//! (the `ModelUtil` statics that need no `ModelManager`/instance
//! reconstruction). Every other fixture is a real fixture — loaded,
//! decoded, and judged — but its op is reported `unsupported`, never
//! silently skipped and never counted as a pass or a fail (`report.rs`).
//! Growing that set (adding `ModelManager`/introspection/instance op
//! support to `decode.rs` and `ops.rs`) is exactly how a later porting task
//! extends this harness's coverage; the fixture loading, comparison and
//! reporting machinery does not change.

mod compare;
mod decode;
mod fixture;
mod ops;
mod report;
mod self_test;

use std::env;
use std::path::{Path, PathBuf};

/// Finds the oracle's `fixtures` directory (see the module doc).
fn find_fixtures_dir() -> Option<PathBuf> {
    if let Ok(configured) = env::var("CONCERTO_ORACLE_FIXTURES") {
        let path = PathBuf::from(configured);
        let candidate = if path.file_name().and_then(|n| n.to_str()) == Some("fixtures") {
            path
        } else {
            path.join("fixtures")
        };
        return Some(candidate);
    }

    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    // concerto-core/ -> concerto-rust/
    let repo_root = manifest_dir.parent()?;
    // concerto-rust/ -> the workspace directory that holds the sibling
    // checkouts (this repo may itself be a git worktree of concerto-rust
    // under <workspace>/wt/<task>/concerto-rust, in which case the sibling
    // `concerto` checkout is not next to it — both layouts are tried).
    let siblings = [
        repo_root.join("../concerto/migration/oracle/fixtures"),
        repo_root.join("../../concerto/migration/oracle/fixtures"),
    ];
    siblings.into_iter().find(|p| p.is_dir())
}

#[test]
fn replays_the_oracle_corpus() {
    let Some(fixtures_dir) = find_fixtures_dir() else {
        eprintln!(
            "oracle harness: no fixture corpus found (set CONCERTO_ORACLE_FIXTURES, or generate \
             one with migration/oracle/bin/record-all.sh in a sibling `concerto` checkout, see \
             migration/oracle/README.md there) — skipping"
        );
        return;
    };
    if !fixtures_dir.is_dir() {
        eprintln!(
            "oracle harness: {} is not a directory — skipping",
            fixtures_dir.display()
        );
        return;
    }

    run(&fixtures_dir);
}

/// Split out from the `#[test]` so a fixed, hand-authored corpus can drive
/// the same path in this crate's own tests of the harness (see
/// `oracle/self_test.rs`).
fn run(fixtures_dir: &Path) {
    let (fixtures, load_errors) = fixture::load_all(fixtures_dir);
    if fixtures.is_empty() && load_errors.is_empty() {
        eprintln!(
            "oracle harness: {} contains no fixtures — skipping",
            fixtures_dir.display()
        );
        return;
    }

    let mut recorder = report::Recorder::new(fixtures_dir.to_path_buf(), load_errors);
    for fx in &fixtures {
        let dispatch = ops::exec(&fx.op, &fx.inputs);
        let verdict = compare::judge(fx, dispatch);
        recorder.record(fx, verdict);
    }

    let report = recorder.finish();
    let report_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("target")
        .join("oracle-report.json");
    report.write(&report_path);
    report.assert_no_regressions();
}
