<!-- Final PR description for accordproject/concerto-rust#78, written by P5-03 (#74). -->
# Tracks #29
This is the integration PR for the concerto-rust side of the migration that makes `@accordproject/concerto-core` run on a Rust engine (plan: #29). Each task was reviewed on its own draft PR against `claude/tender-pascal-ocwf9q` and merged into it; this PR takes the finished branch into `main`. The TypeScript views that call this engine are in accordproject/concerto#1327.

**In short:** `concerto-core` grows from the GSoC 2026 model validator into the engine behind concerto-core. It now covers:
- introspection;
- semantic model validation at parity with `@accordproject/concerto-core@5.0.0`;
- instance validation and serialisation;
- the decorator command set (DCS) operations;
- a message catalogue.

It is built on a generated metamodel crate and a proc-macro crate, and exposed to JavaScript through the new `concerto-wasm` binding. A native oracle harness replays 16,242 recorded v5.0.0 behaviours: 13,921 pass, 0 fail, and the rest are ops that stay in TS.

### Changes

**`concerto-metamodel` (P1-01)**
- `build.rs` downloads the concerto, decorator and vocabulary metamodels and `concerto@1.0.0` at a **pinned tag, checked against a SHA-256** (D4), into `OUT_DIR`. When there is no network it uses the vendored copies in `vendor/`, so an offline build works.
- **Native Rust codegen** replaces the `npx` concerto-cli step. Abstract types become `$class`-tagged serde enums, and `Debug` is derived (#24). The crate is aligned with TS `^3.17`.
- Tests check that the corpus ASTs round-trip.

**`concerto-macros` (P1-03, D5)**
- A proc-macro crate, re-exported as `concerto_core::derive`. It provides derives for the `Named`, `FullyQualified`, `Decorated`, `HasValidators`, `Typed`, `Validate` and `DeclarationKind` traits, plus an error-builder macro.
- It replaces the repeated hand-written impls (`name()` ×5, nine-arm `Property` matches ×6, `IllegalModel{..}` ×32). `scripts/dup-impls.py` enforces the thresholds.

**`concerto-core`**
- **Core on the metamodel (P1-02).** Newtypes over `mm::*`. The root and decorator models deserialise into `mm::Model`, and the hand-built structs are gone. P5-06d adds a typed AST path (`introspect/typed_ast.rs`) that falls back to the `serde_json::Value` path for anything it does not cover.
- **Graph and handles (P1-04).** A stable `DeclId` / `PropId` / `ModelFileId` arena in `ModelManager`, and a `ResolutionContext` for collaborator calls.
- **Error contract (P1-05).**
  - `error/`: `{kind, code, params, location}` errors, with the message templates ported from `messages/en.json` and the inline TS templates into `error/catalogue.rs`, each with a golden test.
  - `ecma.rs`: JS number and string formatting.
  - The `regress` crate handles ECMAScript regular expressions.
- **Introspection parity (P2-01 to P2-11).** `model_util.rs`, `introspect/`, `model_manager.rs`, `semver_range.rs` and `validation.rs`. Every TS `validate()` has a Rust counterpart or a written reason in the P2-09 gap audit. The gaps listed in plan §1.2 are closed:
  - super-type kind compatibility and identity rules;
  - the implicit `Concept` super type and the system fields;
  - enum and scalar rules and `defaultValue` checks;
  - regex dialect and flags;
  - `Decorator.validate`, and duplicate decorators on every node type;
  - map key and value types (#25);
  - semver and `concertoVersion`;
  - the exact `ID_REGEX`;
  - error locations and first-error order.
  - Import order is relaxed (#26): all files are added, then validated, and rolled back on error.
- **Instance layer (P3-01 to P3-04, D3).** `instance/` has the populator, generator, validator, serializer, resource and resource id, factory, dayjs-compatible dates and `value.rs`.
  - It folds in and fixes concerto-validate-rs: super-type properties are merged transitively, abstract and nested `$class` values are checked, and Long, DateTime, relationships, enums, maps and scalars are supported.
  - `DeserializeOptions` and `STRICT_VALIDATE_OPTIONS` implement accordproject/concerto#1273.
  - `Diagnostic` / `ValidationResult` / `validate_instance(_or_throw)` implement accordproject/concerto#1239.
  - `validate_metamodel` / `validate_ast` are rebuilt on the strict preset. The test cases from concerto-validate-rs are ported here with citations.
- **Decorator command sets (P4-09).** `dcs/`: the DCS converter, the decorator manager operations, the extractor and YAML quoting.
- **Tests.**
  - Unit tests are ported from the TS introspect tests.
  - `tests/oracle/` is the native oracle harness (P1-07). It replays `migration/oracle/fixtures` from a concerto checkout (`CONCERTO_ORACLE_FIXTURES`), compares outcome and exception class (P5-09), attributes each fixture to a seam-ledger owner, and fails on any regression against `tests/oracle/baseline.tsv`.
  - `tests/typed_ast/` is the P5-06d drift guard. `tests/derive.rs` and `tests/traits.rs` cover the macros.
  - `MUTANTS.md` records the `cargo-mutants` results for the validation modules (P5-06).

**`concerto-wasm` (new, outside the cargo workspace; P4-01, #28)**
- `wasm-bindgen` bindings, pinned `=0.2.128`. The `ModelManagerHandle` class exposes 112 engine operations to JS, plus a structured error type that the concerto shim maps to TS exception classes.
- `build.sh` builds the module and writes an npm package (`pkg/`, `@accordproject/concerto-engine`) that **instantiates synchronously from inlined bytes**, as CommonJS, ESM and a web build. A size budget is checked.
- `scripts/` has the Node and headless-Chromium smoke tests. `results/` holds the boundary-cost measurements.
- `spikes/wasm/` is the P4-01 spike: browser sync-compile limits and boundary cost.

**Docs and rulebook**
- `PORTING.md`: the rulebook every task followed.
  - Mapping TS to Rust: sum types plus traits, newtypes, views and snapshots.
  - The error contract.
  - Semantics: JS numbers, regex, dates, `null` vs `undefined`, `ID_REGEX`, D6, and order (no `HashMap` iteration on observable paths).
  - Module layout, testing, porting discipline, and a worked example.
  - The P5-03 review adds a §7.1 rule: no catch-all fallback from a view's engine call to the TS body (#262).
- `DIVERGENCES.md`: every known behaviour difference from v5.0.0.
  - DV-001 to DV-019, each categorised as engine, ts-bug, D6 or maintainer-accepted, with its evidence.
  - DV-015, DV-017, DV-018 and DV-019 are accepted divergences.
- `docs/public-api.md`: the standalone Rust interface (D11). In scope: JSON AST loading, introspection, semantic validation, and instance validation with diagnostics. Named follow-ups: CTO parsing, typed instances and JSON generation, and sample generation.
- `benches/`: criterion benchmarks of Rust against TS on the same models (P5-04, P5-06x). They are not a workspace member.

**CI**
- `ci.yml` also runs on the integration branch. It adds the concerto-wasm checks: `cargo fmt --check` and wasm32 clippy with `-D warnings`.
- `conformance-test.yml` patches the harness's concerto-rust dependency to the checked-out `concerto-core` (P0-07). The updated concerto-conformance harness is merged upstream (accordproject/concerto-conformance#37).

### Evidence (P5-03 final review at `e50f0e3`, and the P5-01 gate)
- **Oracle, native:** `CONCERTO_ORACLE_FIXTURES=… cargo test -p accordproject-concerto-core --test oracle`, run on the canonical corpus plus the supplement, with the CTO cache rebuilt and the ledger present. Result: 16,242 fixtures, 13,921 pass, 0 fail, 2,321 unsupported, 0 unowned, 0 regressions against `baseline.tsv`.
- **Oracle, WASM/JS binding:** 16,242/16,242 agree (P5-01 gate).
- **Rust test strength (P5-01 gate):**
  - llvm-cov line coverage of `concerto-core` is 94.43%, against a target of 90% or more.
  - `cargo-mutants` catches 358 of 420 mutants in the validation modules (94.5%), against a target of 85% or more.
- **Conformance:** 75/75 scenarios pass locally against concerto-conformance `9339642`. The `conformance` job on this PR is green.
- **CI on this PR:** Concerto Rust (stable, beta and nightly) and conformance are green. **DCO is red** (see Flags).

### Flags
- **DCO: #260 (blocker, maintainer decision).** Merge commit `5328092` has no `Signed-off-by`. With more than 250 commits, the DCO app cannot evaluate this PR.
- **CI does not run the oracle.** The corpus lives in a concerto checkout and a draft release, so `ci.yml` sets `CONCERTO_ORACLE_SKIP=1`, and `replays_the_oracle_corpus` reports as ignored. The oracle evidence above comes from local and gate runs.
- **Ships with accordproject/concerto#1327.** concerto's CI cannot build this engine yet: #259 is a blocker on that PR. The npm package `@accordproject/concerto-engine` is linked locally and not published (D9). Publishing, and crates.io (#27), are out of scope.
- **Performance:** through concerto-core's public API the engine is slower than TS. After P5-06d, load is about 15× and load plus validate about 8× on the concerto-core test data. Crate-level criterion results are in `benches/results/`. The maintainer accepted current performance for this release.
- **Open review findings from P5-03, escalated to a third review** (verdicts are posted on each issue and await coordinator confirmation):
  - #263: `ModelFile::get_external_imports` returns a `HashMap`, so JS sees keys in a different order from v5.0.0. This violates PORTING 3.7. Upheld; the fix is an `IndexMap`.
  - #262: catch-all engine-call fallbacks on the TS side. Upheld. PORTING §7.1 now forbids them.
  - #261: seam-ledger rows marked RUST with no engine call. Upheld. It affects the §0.4 figure reported on accordproject/concerto#1327.
- **Other follow-ups:**
  - #264: DIVERGENCES categories (DV-009, DV-004).
  - #265: 211 Rust-owned fixtures, 171 of them `addCTOModel` with `addMetamodel`, are never compared natively.
  - #266: the TS-only remainder.
- **Breaking changes** are catalogued in accordproject/concerto `migration/BREAKING-CHANGES-PLAN.md`. RB is the plan for the Rust crate's 1.0 API.
- **concerto-validate-rs** has no PR. The maintainer decided to leave it untouched and archive it after the migration. Its logic and tests now live in `concerto-core::instance` (D3, P3-04).

### Screenshots or Video
N/A

### Related Issues
- Issue #29 (plan), task issues #30–#266, final review #74; folded-in issues #24, #25, #26 and #28
- Pull Request accordproject/concerto#1327 (the TS views and the migration harness)
- Pull Request accordproject/concerto-conformance#37 (the harness update, merged)

### Author Checklist
- [ ] Ensure you provide a [DCO sign-off](https://github.com/probot/dco#how-it-works) for your commits using the `--signoff` option of git commit. (Not yet: see #260.)
- [x] Vital features and changes captured in unit and/or integration tests (the ported TS tests, golden message tests and the native oracle harness)
- [x] Commits messages follow [AP format](https://github.com/accordproject/techdocs/blob/master/DEVELOPERS.md#commit-message-format)
- [x] Extend the documentation, if necessary (`PORTING.md`, `DIVERGENCES.md`, `docs/public-api.md`, `concerto-wasm/README.md`)
- [ ] Merging to `main` from `fork:branchname`

🤖 Generated with [Claude Code](https://claude.com/claude-code)
