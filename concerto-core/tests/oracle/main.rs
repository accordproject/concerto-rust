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
//! 1. `$CONCERTO_ORACLE_FIXTURES`, or PORTING.md OD-7's
//!    `$CONCERTO_ORACLE_DIR`, if set (a `migration/oracle/fixtures`
//!    directory, or a directory containing one);
//! 2. `../concerto/migration/oracle/fixtures` next to this repository
//!    checkout (the layout `migration/oracle/README.md`'s own commands and
//!    this repo's `PORTING.md` assume: `concerto` and `concerto-rust`
//!    cloned as siblings), then `../../concerto/migration/oracle/fixtures`
//!    (a worktree one level down, `<workspace>/wt/<task>`).
//!
//! **A missing corpus fails the test.** No corpus found, a configured path
//! that is not a directory, and a directory with no fixtures in it all
//! panic, naming what was tried. A run that replayed nothing must never
//! read as `ok`: a git worktree that was not a direct sibling of `concerto`
//! once passed a merge-stage run that way without loading a single fixture.
//!
//! To run the rest of the suite without the corpus, opt out explicitly:
//!
//! ```text
//! CONCERTO_ORACLE_SKIP=1 cargo test --workspace
//! ```
//!
//! `concerto-core`'s build script turns `CONCERTO_ORACLE_SKIP=1` into the
//! `concerto_oracle_skip` cfg, which marks this test `#[ignore]`, so the run
//! prints `replays_the_oracle_corpus ... ignored, CONCERTO_ORACLE_SKIP=1: ...`
//! and counts it under `ignored`, not `passed`. concerto-rust's CI has no
//! corpus and sets it (`.github/workflows/ci.yml`). Any other value, or
//! none, leaves the test on.
//!
//! # CTO text
//!
//! CTO parsing stays in JS, so every CTO text a model-manager recipe or op
//! parses is looked up in the CTO -> AST cache that task P1-07a's
//! `migration/oracle/bin/build-cto-cache.js` writes next to `fixtures/`
//! (`$CONCERTO_CTO_CACHE` overrides it; `cto_cache.rs`). A text with no
//! entry is a harness error.
//!
//! # What is compared
//!
//! `ops.rs`'s module doc lists the ops this harness dispatches, and
//! `recipe.rs` how a fixture's model managers, model files, declarations
//! and properties are rebuilt on the Rust engine. Every fixture is loaded,
//! decoded and judged; one whose op, or some input, has no Rust counterpart
//! yet is reported `unsupported`, with the reason and the owning task,
//! never silently skipped and never counted as a pass or a fail
//! (`report.rs`). Failures are reported per rule and per fixture, and
//! judged against `baseline.tsv` (`report.rs`).
//!
//! # Running part of the corpus
//!
//! `ORACLE_OP=<prefix>` (PORTING.md OD-7) runs only the fixtures whose op
//! starts with the prefix, e.g. `ORACLE_OP=ModelUtil.` or
//! `ORACLE_OP=ModelManager.addCTOModel`:
//!
//! ```text
//! ORACLE_OP=ModelUtil. cargo test -p accordproject-concerto-core --test oracle -- --nocapture
//! ```

mod compare;
mod cto_cache;
mod decode;
mod fixture;
mod ledger;
mod ops;
mod recipe;
mod report;
mod self_test;

use std::env;
use std::path::{Path, PathBuf};

/// What an op needs besides its own inputs.
pub struct Harness {
    /// The P1-07a CTO -> AST cache, when one was built.
    pub cache: Option<cto_cache::CtoCache>,
    /// The seam ledger, for the owning task in an `unsupported` reason.
    pub ledger: ledger::Ledger,
}

/// Finds the oracle's `fixtures` directory (see the module doc). `Ok` is the
/// directory to replay and where it came from; `Err` lists the sibling paths
/// tried, for the failure message.
fn find_fixtures_dir() -> Result<(PathBuf, String), String> {
    // `CONCERTO_ORACLE_DIR` is PORTING.md OD-7's name for the same setting
    // (the `migration/oracle` directory, which contains `fixtures`).
    for var in ["CONCERTO_ORACLE_FIXTURES", "CONCERTO_ORACLE_DIR"] {
        if let Ok(configured) = env::var(var) {
            let path = PathBuf::from(configured);
            let candidate = if path.file_name().and_then(|n| n.to_str()) == Some("fixtures") {
                path
            } else {
                path.join("fixtures")
            };
            // A configured path is used as is, never falling back to the
            // sibling search: if it holds no corpus, the test fails.
            return Ok((candidate, format!("${var}")));
        }
    }

    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    // concerto-core/ -> concerto-rust/
    let repo_root = manifest_dir.parent().unwrap_or(&manifest_dir);
    // concerto-rust/ -> the workspace directory that holds the sibling
    // checkouts (this repo may itself be a git worktree of concerto-rust
    // under <workspace>/wt/<task>, in which case the sibling `concerto`
    // checkout is not next to it — both layouts are tried).
    let siblings = [
        repo_root.join("../concerto/migration/oracle/fixtures"),
        repo_root.join("../../concerto/migration/oracle/fixtures"),
    ];
    match siblings.iter().find(|p| p.is_dir()) {
        Some(found) => Ok((found.clone(), "a sibling `concerto` checkout".to_owned())),
        None => Err(siblings
            .iter()
            .map(|p| format!("  {}", p.display()))
            .collect::<Vec<_>>()
            .join("\n")),
    }
}

/// How to get a corpus, or opt out, for every "no corpus" failure.
const NO_CORPUS_HELP: &str = "Point CONCERTO_ORACLE_FIXTURES at a corpus \
     (<concerto>/migration/oracle/fixtures; generate one with \
     migration/oracle/bin/record-all.sh, see migration/oracle/README.md in the `concerto` \
     checkout), or set CONCERTO_ORACLE_SKIP=1 to report this test as ignored instead.";

#[test]
#[cfg_attr(
    concerto_oracle_skip,
    ignore = "CONCERTO_ORACLE_SKIP=1: the oracle corpus was not replayed"
)]
fn replays_the_oracle_corpus() {
    let (fixtures_dir, source) = match find_fixtures_dir() {
        Ok(found) => found,
        Err(tried) => panic!(
            "oracle harness: no fixture corpus found, so no oracle fixture was replayed. \
             CONCERTO_ORACLE_FIXTURES and CONCERTO_ORACLE_DIR are unset, and none of these \
             exists:\n{tried}\n{NO_CORPUS_HELP}"
        ),
    };
    assert!(
        fixtures_dir.is_dir(),
        "oracle harness: the corpus directory from {source}, {}, is not a directory, so no \
         oracle fixture was replayed. {NO_CORPUS_HELP}",
        fixtures_dir.display()
    );
    println!(
        "oracle harness: corpus {} (from {source})",
        fixtures_dir.display()
    );
    run(&fixtures_dir);
}

/// Split out from the `#[test]` so a fixed, hand-authored corpus can drive
/// the same path in this crate's own tests of the harness (see
/// `oracle/self_test.rs`).
fn run(fixtures_dir: &Path) {
    let (fixtures, load_errors) = fixture::load_all(fixtures_dir);
    // An empty corpus directory replays nothing, which must fail rather than
    // report `test result: ok` (task P1-07 review), wherever it was found.
    assert!(
        !(fixtures.is_empty() && load_errors.is_empty()),
        "oracle harness: {} contains no fixtures, so no oracle fixture was replayed. \
         {NO_CORPUS_HELP}",
        fixtures_dir.display()
    );

    let filter = env::var("ORACLE_OP").ok().filter(|f| !f.is_empty());
    let harness = Harness {
        cache: cto_cache::CtoCache::locate(fixtures_dir),
        ledger: ledger::Ledger::load(fixtures_dir),
    };
    match &harness.cache {
        Some(cache) => println!("oracle harness: CTO cache {}", cache.dir().display()),
        None => println!(
            "oracle harness: no CTO cache next to the corpus (build it with \
             `node migration/oracle/bin/build-cto-cache.js`); every CTO text is a harness error"
        ),
    }

    let mut recorder =
        report::Recorder::new(fixtures_dir.to_path_buf(), filter.clone(), load_errors);
    for fx in &fixtures {
        if filter
            .as_ref()
            .is_some_and(|f| !fx.op.starts_with(f.as_str()))
        {
            continue;
        }
        let dispatch = ops::exec(&harness, &fx.op, &fx.inputs);
        let verdict = compare::judge(fx, dispatch);
        recorder.record(fx, verdict, &harness.ledger);
    }

    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let baseline_path = manifest_dir
        .join("tests")
        .join("oracle")
        .join("baseline.tsv");
    let baseline = report::read_baseline(&baseline_path);
    let report = recorder.finish(&baseline);
    let report_path = manifest_dir
        .join("..")
        .join("target")
        .join("oracle-report.json");
    report.write(&report_path);
    if env::var("ORACLE_UPDATE_BASELINE").is_ok_and(|v| v == "1") {
        report.write_baseline(&baseline_path);
        return;
    }
    report.assert_no_regressions();
}
