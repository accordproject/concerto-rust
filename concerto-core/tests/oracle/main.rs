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
//! judged against `known-failures.tsv` (`report.rs`).
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

/// Where the corpus directory came from: whether a developer or CI job
/// pointed at it explicitly, or this harness found it itself by convention.
/// The two are judged differently (see `replays_the_oracle_corpus`): a
/// path nobody asked for is fine to skip when it is absent, but a path
/// someone configured is a promise that a corpus is there, and a broken
/// promise is a harness failure, not a quiet pass.
enum Located {
    /// `$CONCERTO_ORACLE_FIXTURES` was set to this path.
    Explicit(PathBuf),
    /// Found by searching next to this checkout; nobody configured it.
    Discovered(PathBuf),
}

/// Finds the oracle's `fixtures` directory (see the module doc).
fn find_fixtures_dir() -> Option<Located> {
    // `CONCERTO_ORACLE_DIR` is PORTING.md OD-7's name for the same setting
    // (the `migration/oracle` directory, which contains `fixtures`).
    if let Ok(configured) =
        env::var("CONCERTO_ORACLE_FIXTURES").or_else(|_| env::var("CONCERTO_ORACLE_DIR"))
    {
        let path = PathBuf::from(configured);
        let candidate = if path.file_name().and_then(|n| n.to_str()) == Some("fixtures") {
            path
        } else {
            path.join("fixtures")
        };
        return Some(Located::Explicit(candidate));
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
    siblings
        .into_iter()
        .find(|p| p.is_dir())
        .map(Located::Discovered)
}

#[test]
fn replays_the_oracle_corpus() {
    let Some(located) = find_fixtures_dir() else {
        eprintln!(
            "oracle harness: no fixture corpus found (set CONCERTO_ORACLE_FIXTURES, or generate \
             one with migration/oracle/bin/record-all.sh in a sibling `concerto` checkout, see \
             migration/oracle/README.md there) — skipping"
        );
        return;
    };

    let (fixtures_dir, explicit) = match located {
        Located::Explicit(dir) => (dir, true),
        Located::Discovered(dir) => (dir, false),
    };

    if !fixtures_dir.is_dir() {
        // A path this harness found on its own not existing just means no
        // corpus was ever generated there — fine to skip. A path someone
        // set CONCERTO_ORACLE_FIXTURES to is a promise that a corpus lives
        // there, and a missing directory breaks that promise: the run must
        // fail loudly rather than silently report a pass with nothing run
        // (task P1-07 review).
        assert!(
            !explicit,
            "oracle harness: CONCERTO_ORACLE_FIXTURES was set to {}, but it is not a directory \
             (generate the corpus with migration/oracle/bin/record-all.sh, see \
             migration/oracle/README.md in the `concerto` checkout)",
            fixtures_dir.display()
        );
        eprintln!(
            "oracle harness: {} is not a directory — skipping",
            fixtures_dir.display()
        );
        return;
    }

    run(&fixtures_dir, explicit);
}

/// Split out from the `#[test]` so a fixed, hand-authored corpus can drive
/// the same path in this crate's own tests of the harness (see
/// `oracle/self_test.rs`).
fn run(fixtures_dir: &Path, explicit: bool) {
    let (fixtures, load_errors) = fixture::load_all(fixtures_dir);
    if fixtures.is_empty() && load_errors.is_empty() {
        // Same reasoning as the missing-directory case above: a directory
        // this harness found by convention being empty is not evidence of
        // anything wrong (nobody promised a corpus there), but a directory
        // someone explicitly configured being empty means the harness ran
        // zero fixtures while claiming a corpus was in use — that must fail
        // rather than report `test result: ok` (task P1-07 review).
        assert!(
            !explicit,
            "oracle harness: CONCERTO_ORACLE_FIXTURES ({}) contains no fixtures — a corpus was \
             explicitly configured, so an empty one is a failure, not a skip",
            fixtures_dir.display()
        );
        eprintln!(
            "oracle harness: {} contains no fixtures — skipping",
            fixtures_dir.display()
        );
        return;
    }

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
        recorder.record(fx, verdict);
    }

    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let baseline_path = manifest_dir
        .join("tests")
        .join("oracle")
        .join("known-failures.tsv");
    let baseline = report::read_baseline(&baseline_path);
    let report = recorder.finish(&baseline);
    let report_path = manifest_dir
        .join("..")
        .join("target")
        .join("oracle-report.json");
    report.write(&report_path);
    if env::var("ORACLE_UPDATE_BASELINE").is_ok_and(|v| v == "1") {
        assert!(
            filter.is_none(),
            "ORACLE_UPDATE_BASELINE=1 needs a full run: unset ORACLE_OP"
        );
        report.write_baseline(&baseline_path);
        return;
    }
    report.assert_no_regressions();
}
