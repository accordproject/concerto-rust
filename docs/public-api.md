# The public API of `concerto-core` as a standalone Rust library

**Status: design for maintainer agreement, revision 2.** Task P6-01
(accordproject/concerto-rust#83), plan decision D11
(accordproject/concerto-rust#29).

- **Revision 1** was the early draft, merged through #161 at `b211ae9`. It was
  written before P5-03 (#74) and changed no code.
- **Revision 2** is this one. P5-03 is closed and P5-10d (#285) has merged, so
  the audit is redone at the integration head `af207c5`. The revision takes in
  the maintainer's input of 2026-09-27 on #83: the integration branch has made
  `concerto-core` JS-centric compared with `main`, and the design must isolate
  the JS-compatibility layer from the crate's public API. It also takes in the
  P5-07 breaking-changes plan (concerto `migration/BREAKING-CHANGES-PLAN.md`,
  rows BR-01 to BR-11) and the P5-06d typed deserialisation (#239).

This revision ships with the code for section 7's steps 0 to 2 and the
naming half of step 3:

- **Step 0:** the 15 public items that had no doc comment now have one, and
  `lib.rs` sets `#![warn(missing_docs)]`.
- **Step 1:** the `js-compat` feature. The JS object model, the `$$` tag
  encoding, the TS class mapping and the seam are public only with it, so
  core's default public API has no JS type (exit condition 3).
- **Step 2:** `main`'s names are back: the inherent `name`, `type_name`,
  `decorators`, `declaration_kind` and `scalar_type`, the functions
  `model_util::{short_name, namespace_of, qualify}`, and a `prelude`.
- **Step 3, naming:** `ErrorKind::{Error, JsTypeError, JsRangeError}` are
  `InvalidArgument`, `MalformedInput` and `RecursionLimit`, and
  `ErrorKind::ts_class` is behind `js-compat`.

concerto-wasm enables `js-compat` and its exported JS API is unchanged (it
still reports the kinds to JS by their old names). The comment on #83 of
2026-09-28 requires a maintainer decision before any change to public naming
or error types beyond this note. The rest of section 7 is still a proposal,
and section 8 lists what is still open.

Every number and name below describes `claude/tender-pascal-ocwf9q` at
`af207c5`, unless it says otherwise. "`main`" means concerto-rust `main` at
`a80e562`, the commit the maintainer's input compares against. Today's
`c582700` differs from it only in a CI workflow file.

---

## 1. Scope

D11 makes `concerto-core` usable from Rust without the TS wrapper.

| In scope (this note) | Out of scope (named follow-ups) |
|---|---|
| Loading models from their JSON AST | CTO parsing (possibly through `concerto-tree-sitter`) |
| Introspection: declarations, properties, types, inheritance | Typed instance objects and JSON generation (`Serializer`, `Factory`, `Resource`) |
| Semantic validation of the loaded models | Sample generation (`InstanceGenerator`) |
| Instance validation with diagnostics: the accordproject/concerto#1273 options and the accordproject/concerto#1239 collect-all result | |

The decorator command sets (`dcs`) are in neither column of D11. This note
keeps them outside the stable surface for now (Q3).

The maintainer's requirements of 2026-09-27 are the design constraints. Each
one is answered in the section given here:

| # | Requirement | Answer |
|---|---|---|
| R1 | Keep the core crate's public API Rust-idiomatic: typed structs, Rust error types, no `undefined` or JS value model. | Sections 2, 5 and 6 |
| R2 | Put the JS-compat layer behind a `js-compat` feature or in a separate crate (for example `concerto-core-js`) that concerto-wasm depends on. Evaluate both. | Section 4 |
| R3 | concerto-core's JS behaviour stays byte-identical through WASM. Oracle, fuzz and conformance parity must not regress. | Sections 4.6 and 7 |
| R4 | Decide whether to restore `main`'s removed method names as compatibility shims or deprecations. | Section 5.8 |
| R5 | Take in the P5-07 plan (its Rust-crate-API-only rows especially) and P5-06d (#239). | Section 6 |

The related tasks are:

- P6-02 (#84): the native example and the docs;
- P6-03 (#85): `cargo public-api` and semver checks in CI;
- P6-04 (#273), which depends on this task;
- PORTING.md section 4: the guard that keeps WASM and JS-facing types out of
  core's public API.

---

## 2. What the stable surface promises

These are the guarantees the stable items (section 5) carry once section 7
is done. Items outside the stable surface carry none of them.

1. **Semver.** A breaking change to a stable item needs a minor-version bump
   while the crate is `0.x`, and a major bump after `1.0`. P6-03 enforces this
   with `cargo public-api` and `cargo semver-checks`, run on
   `accordproject-concerto-core` alone with default features.
2. **Same verdict as the TS reference.** Loading, semantic validation and
   instance validation accept and reject what `@accordproject/concerto-core@5.0.0`
   accepts and rejects (D10), and each rejection has the `ErrorKind` of the TS
   exception class (section 5.6). The recorded divergences in
   `DIVERGENCES.md`, and the BC rows of the P5-07 plan once they ship, are
   part of the contract. Following the maintainer's error-parity decision of
   2026-09-27, the class is part of the verdict and the wording is not. The
   message text does match TS today, and the oracle pins it through the WASM
   binding (R3), but native callers get no promise about it.
3. **Stable codes, not stable wording.** The catalogue key (`Error::code`),
   the #1273 `DetailCode` and the #1239 `DiagnosticCode` are stable
   identifiers, safe to match on. A message can change in a minor release.
   When BC-38 settles the public code format (the conformance `@rule` IDs), it
   is exposed through the same `code()` accessor.
4. **Deterministic order.** Iterators and `Vec`s come back in the TS order:
   files in load order, declarations and properties in AST order, and
   diagnostics in walk order (PORTING.md 3.7).
5. **No panics on input.** No malformed AST or instance makes a stable entry
   point panic. Unbounded recursion becomes an error, never a native stack
   overflow (PORTING.md 2.5).
6. **Thread safety.** `ModelManager`, `ModelFile` and the error type are
   `Send + Sync`, and the error type is `std::error::Error + 'static`. P6-03
   adds a static assertion for this.
7. **No JS in the signatures.** No stable item names `wasm-bindgen`,
   `js-sys`, or a type that models a JS value (`undefined`, a JS `Map`, a
   dayjs object, a `$$`-tagged `Value`), a JS option bag, a TS class name or
   a JS exception class. `cargo public-api` output is checked for this
   (section 4.6).
8. **MSRV** is the workspace `rust-version` (1.88 today). Raising it is a
   minor-version change.

---

## 3. Audit at `af207c5`

### 3.1 Method

- **Items:** the rustdoc `all.html` of `cargo doc -p accordproject-concerto-core --no-deps`.
- **Methods:** a count of `pub fn` outside the `*_tests.rs` files.
- **Consumers:** the `concerto_core::` paths used by `concerto-wasm/src/lib.rs`
  (5,949 lines), by the concerto-conformance Rust harness
  (`semantic/features/support/rust/cucumber_tests/src/steps.rs`; it uses only
  `ModelManager::new`, `add_model`, `validate_models` and the error's
  `Display`), by `concerto-validate-rs` (`validate_ast`) and by `benches/`.
- **Docs:** `cargo rustc -p accordproject-concerto-core --lib -- -W missing_docs`,
  and rustdoc's own warnings.
- **`main`:** `git show a80e562:concerto-core/src/...`.

### 3.2 Size

| | `main` (`a80e562`) | `af207c5` |
|---|---|---|
| `concerto-core/src` lines (`.rs`, with the in-file tests) | 3,686 | 42,882 |
| Public modules | 9: 5 top-level (`error`, `introspect`, `model_manager`, `model_util`, `rootmodel`), 4 under `introspect` | 30: 8 top-level, 2 under `dcs`, 12 under `instance`, 8 under `introspect` |
| Structs / enums / traits | 5 / 6 / 0 | 44 / 20 / 10 |
| Free functions / constants | 8 / 0 | 88 / 13 |
| `pub fn`, free and methods, outside private modules and test files | 65 | 382 |
| `pub fn get_*` | 2 (`get_declaration`, `get_all_properties`) | 44 |

Most of the growth is the faithful port the plan asked for: the TS rules, the
message catalogue, the instance validator and the oracle-driven fixes. That
logic belongs in core. What does not belong in core's public API is the part
that exists only because a JS caller needs it. Section 3.3 separates the two.

### 3.3 Classification by module

Each item falls into one of five groups:

- **Stable:** part of the D11 surface, possibly under a new name (section 5).
- **JS object model:** models a JS value or a TS instance object. It leaves
  core (section 4).
- **Seam:** has no JS type in its signature, but exists only so the TS views
  can be built on the Rust graph. It moves behind the `js-compat` feature
  (section 4).
- **Follow-up:** belongs to an out-of-scope D11 area. It leaves the default
  public API until its follow-up designs it.
- **Internal:** used only inside the crate or its tests. It becomes
  `pub(crate)`.

| Module | Items | Group | Notes |
|---|---|---|---|
| `ModelManager`: `new`, `add_model`, `add_models`, `model_file(s)`, `get_declaration`, `get_type_declaration`, `resolve_type`, `resolve_type_name`, `is_assignable_to`, `derives_from`, `get_all_properties`, `get_property`, `get_own_properties`, `get_super_type(_declaration)`, `get_all_super_type_names`, `get_direct_subclasses`, `get_assignable_class_declarations`, `get_*_declarations`, `identifier_field_name`, `is_identified`, `is_system_identified`, `get_ast`, `filter` | about 35 | Stable | Renamed as in 5.5, with `main`'s names kept (5.8). |
| `ModelManager`: `add_model_with_definitions`, `update_model_file`, `delete_model_file`, `update_external_models` (and `ModelFileSource`), `resolve_meta_model`, `get_models`, `model_file_fully_qualified_type_name`, `get_nested_property`, `property_default_value` | 10 | Stable, second tier | For a native caller that manages files. |
| `ModelManager`: `generation`, `file`, `declaration`, `property`, `declaration_ids`, `property_ids`, `class_declarations`, `model_file_of`, `parent_of`, `model_file_id`, `declaration_id`, `is_type_assignable_to`, `get_assignable_concrete_types` | 13 | Seam | The handle API P1-04 built for the views. `generation()` exists so that a JS view can tell a stale snapshot. |
| `ModelManager`: `decorator_validation`, `set_decorator_validation`, `dangerously_allow_reserved_system_type_names_in_user_models` and its setter, `metamodel_validation`, `set_metamodel_validation`, `validate_ast` | 7 | Stable | The options become builder options (5.2). |
| `model_manager::{DeclId, ModelFileId, PropId}` | 3 | Stable type, Seam methods | PORTING.md section 4 keeps the ids in core. `from_index` and `index` exist for JS. |
| `model_manager::{ResolutionContext, ValidatedElement, Node}` | 3 | Seam | The collaborator fallback (PORTING.md 1.4). `Node::Primitive` models a JS string answer. |
| `validation`: `ModelManager::validate_models`, `validate_model_file` | 2 | Stable | `validate_models` is the conformance harness's entry point. |
| `validation`: `validate_detached_*`, `validate_map_key`, `validate_map_value` | 6 | Seam | For views over a file or declaration not yet in the arena. PORTING.md section 4 calls `validation.rs` "crate-private", but it is `pub`. |
| `introspect::{Declaration, ClassDeclaration, ClassKind, EnumDeclaration, MapDeclaration, ScalarDeclaration, Property, Import, ModelFile, Decorator, DecoratorArgument, TypeReferenceArgument}` and their read-only methods | about 110 | Stable | The introspection surface. |
| `introspect` traits: `Named`, `FullyQualified`, `Typed`, `Decorated`, `DeclarationKind` | 5 | Stable | Kept for generic code, but the methods are inherent again (5.8). `FullyQualified`'s associated `Error` exists only because the JS context can fail. |
| `introspect` traits: `HasValidators`, `Validate` | 2 | Internal | Load-time check plumbing. |
| `introspect`: `ProcessedField`, `ProcessedProperty`, `ProcessedScalar`, `ProcessDecision`, `field::process`, `property::process`, `ScalarDeclaration::process`/`validate_new`/`build_standalone`, `field::scalar_to_field_ast`, `ClassDeclaration::process_decision`/`kinds_compatible`/`identifier_redeclare_conflict`/`is_kind`/`to_string`, `EnumDeclaration::to_string`, `ModelFile::check_constructor_arguments`, `Property::check_bound_validators`, `Decorator::validate`, `Decorator::js_name` (a TS `undefined` name) | about 20 | Seam | The constructor and `process()` ports of PORTING.md 1.2. They read `serde_json::Value` ASTs without `$class` that only the TS views produce. |
| `introspect::validators::{Validator, NumberValidator, StringValidator, CollectionSizeValidator}` | 4 | Stable type, Seam constructors | Reading bounds and the regex is introspection. `new(&dyn ValidatedElement, …)` and `validate(…)` are view plumbing. |
| `introspect::DecoratorValidationOptions` | 1 | Stable | The `decoratorValidation` option. |
| `ModelFile::from_json`, `ModelFile::from_json_text` | 2 | Stable | `from_json_text` is the P5-06d typed fast path (#239), 1.5 to 2.7 times faster than parsing a `Value` first. |
| `model_util::*` | 19 functions, 2 constants, `SemVer`, `ParsedNamespace`, `PrereleaseIdentifier` | Split | Stable: `short_name`, `namespace_of`, `qualify` (restored from `main`, 5.8), `parse_namespace`, `is_valid_identifier`, `is_primitive_type`, `is_system_property`, `ParsedNamespace`. Seam: the functions that take a JS-shaped `Option<&str>` or answer a TS type test over a stub (`get_namespace`, `is_enum`, `is_map`, `is_scalar`, `is_valid_map_key*`, `is_valid_map_value`, `import_fully_qualified_names`, `remove_namespace_version_from_fully_qualified_name`, `capitalize_first_letter`, `MAP_KEY_KINDS`, `MAP_VALUE_KINDS`). Internal: `SemVer` and `PrereleaseIdentifier`. |
| `rootmodel::{root_model, root_model_ast, decorator_model, decorator_model_ast}` | 4 | Stable | The system models. |
| `error::{ConcertoError, Result, ContractError, ErrorKind, DetailCode, ValidationDetail}` | 6 | Stable, reshaped | Section 5.6. |
| `error::{ValidatorReport, Renderer, CatalogueEntry, CATALOGUE, catalogue_entry}`, `ErrorKind::ts_class`, `ContractError::pre_port`/`component`/`final_message`/`model_file` | 10 | Seam | The shim's exception mapping (PORTING.md 2.3) and the harness's golden tests. |
| `instance::{validate_instance, ValidateOptions, DeserializeOptions, STRICT_VALIDATE_OPTIONS, Diagnostic, DiagnosticCode, Severity, ValidationResult}`, `ModelManager::validate_instance(_or_throw)`, `ClassDeclaration::validate_instance(_or_throw)` | 12 | Stable, reshaped | Section 5.7. |
| `instance::{validate_metamodel, validate_ast, METAMODEL_NAMESPACE}` | 3 | Stable | `validateAst`. `concerto-validate-rs` is a CLI over it (D3). |
| `instance::validate::{validate_instance_from, validate_property_value, DAYJS_TAG, RELATIONSHIP_TAG, UNDEFINED_TAG, NUMBER_TAG, MAP_TAG, js_map, js_special_number, js_bigint, js_undefined, is_js_undefined}` | 12 | Seam | The `$$` tag encoding of JS values inside `serde_json::Value` (section 4.1). |
| `instance::value::{JsValue, Instance, InstanceKind}`, `instance::dayjs::{Dayjs, UtcOffset}`, `instance::serializer::{Serializer, SerializerOptions}`, `instance::factory::*` (`InstanceEnv`, `NewResourceCheck`), `instance::populator::*`, `instance::generator::*`, `instance::resource::*`, `instance::resource_id::ResourceId` | about 60 | JS object model (and Follow-up) | The state of the TS `Resource` objects, which D7 keeps in TS. |
| `dcs::*`, `dcs::extractor::*`, `dcs::dcsconverter::*` | 25 functions, 7 types, 2 constants | Outside D11 (Q3) | Ports of `DecoratorManager` and `DecoratorExtractor`, with TS-shaped signatures over `serde_json::Value`. |
| `derive` (the `concerto-macros` re-export) | 1 | Internal to the workspace | The derives implement core's own traits on core's own types. |

### 3.4 Findings

**F1. No `concerto-wasm` or JS *crate* type in core.** `cargo tree -p
accordproject-concerto-core -e normal | grep -E 'wasm-bindgen|js-sys|web-sys'`
prints nothing, and no `#[wasm_bindgen]` appears in core.

**F2. JS-modelling types are in core's default public API.** PORTING.md
section 4's check, `grep -rn 'wasm_bindgen\|JsValue' concerto-core/src`,
prints 313 lines in 11 files. All of them are core's own
`instance::value::JsValue`, not `wasm_bindgen::JsValue`, so the letter of the
rule holds and its purpose does not. The same holds for `Dayjs`,
`SerializerOptions` (an `IndexMap<String, JsValue>` option bag), the `$$` tag
constants and helpers, `InstanceKind::ctor` (a TS class name),
`ErrorKind::{JsTypeError, JsRangeError}` and `ErrorKind::ts_class`. The exit
condition "no `concerto-wasm` or JS type appears in core's public API" was
not met at `af207c5`. Section 7's step 1, done in this revision, meets it.

**F3. The binding sets the shape of the surface.** Five traits
(`ResolutionContext`, `ValidatedElement`, `FullyQualified`'s associated
`Error`, `HasValidators`, `Validate`) and the `process`/`Processed*` family
exist so that a JS view can call Rust about an element that is not in the
arena yet. A native caller never needs them.

**F4. Two lookup styles.** Name-keyed (`get_declaration(fqn) ->
Result<&Declaration>`) and handle-keyed (`declaration(DeclId) ->
Option<&Declaration>`) sit side by side, and similar questions return names
(`get_all_super_type_names -> Vec<String>`) or handles
(`get_asset_declarations -> Vec<DeclId>`).

**F5. The naming is inconsistent.** 44 `pub fn get_*` keep the TS names next
to getter-style names on the same types. `add_model` loads a JSON AST, where
TS `addModel` takes CTO text.

**F6. The error type has three overlapping shapes.** `ConcertoError::{TypeNotFound,
IllegalModel, Contract}`. The first two are hand-built and have no catalogue
code; 41 non-test references construct or match them. No enum in the crate is
`#[non_exhaustive]`.

**F7. `missing_docs`.** At `af207c5` the lint gave 15 warnings: 10 enum
variants (`JsValue` 5, `InstanceKind` 3, `UtcOffset` 2), 4 fields of
`GeneratorOptions`, and `ModelManager::generation`, whose doc comment had
drifted onto `decorator_validation`. This revision fixes all 15 and turns the
lint on (section 7, step 0). Rustdoc also reported 55 other warnings: 49
public docs that link to private items and 6 unresolved links. Step 1, done
in this revision, fixes them: rustdoc gives no warning for core with or
without `js-compat`.

**F8. The #1273 options are only reachable through the JS object model.**
`DeserializeOptions` takes effect only inside `JSONPopulator`, through
`Serializer::from_json` with a `SerializerOptions` bag of `JsValue`s. The
native `validate_instance(&Value, &ValidateOptions)` does not take them.
`validate_metamodel` gets them by building a `Serializer` internally, so the
stable `validate_ast` depends on the JS object model today.

**F9. `main`'s public names were moved or renamed (the maintainer's
criticism).** Section 5.8 has the list. `name()`, `type_name()`,
`decorators()`, `declaration_kind()` and `scalar_type()` became trait
methods or changed shape, so callers now need trait imports. The functions
`model_util::{qualify, short_name, namespace_of}` became
`get_fully_qualified_name`, `get_short_name` and `get_namespace(Option<&str>)
-> Result`. `ModelManager::get_all_properties` went from `Vec<&Property>` to
`Vec<(String, Property)>`. `ConcertoError` lost `NamespaceNotFound` and
`ValidationFailed`.

**F10. `ModelManager: Default` gives a manager without the system models.**
This was so on `main` too (`#[derive(Default)]`). `new()` loads the system
models, and `default()` does not, so the two constructors disagree. See Q7.

---

## 4. Isolating the JS-compatibility layer (R2)

### 4.1 What is JS-shaped, and what only looks it

There are three different things behind "JS in core". The design treats
them differently.

| Kind | Where | Size | Treatment |
|---|---|---|---|
| **(a) The JS object model.** Values and objects that exist only in a JS caller: `undefined`, a JS `Map`, a `BigInt`, a dayjs object, a TS `Resource`, and the option bags that carry them. | `instance/value.rs` (375 lines), `dayjs.rs` (748), `serializer.rs` (811), `populator.rs` (867), `factory.rs` (645), `generator.rs` (399), `resource.rs` (178), `resource_id.rs` (579); the `$$` tags in `validate.rs` | about 4,600 lines, plus the tag handling | **Leaves core** (4.4). |
| **(b) ECMAScript semantics that *are* Concerto semantics.** Concerto defines numbers as doubles, string lengths in UTF-16 code units, regexes in the ECMAScript dialect, and the `ID_REGEX`. A Rust caller must get the same verdict as a JS caller for the same model. | `ecma.rs` (336 lines, already private `mod ecma`), `regress`, `ryu-js`, the string validator | – | **Stays in core, private.** It is implementation, not API. |
| **(c) Emulation of TS quirks on malformed input.** How TS treats a numeric name, a truthy non-string, a missing `$class` or a cyclic super type, and the TS exception class each one throws. | `ecma::{to_js_string, is_truthy, to_number}` from `introspect/*`, `validation.rs`, `model_manager.rs`; `ErrorKind::{JsTypeError, JsRangeError}` | spread | **Stays in core, private,** because it decides the verdict for both callers (guarantee 2). Its error kinds get Rust names (5.6). It shrinks as BC-19 and BR-09 land (section 6). |

Kind (a) is what makes the public API JS-centric. Kinds (b) and (c) make the
source look JS-centric, but a native caller never sees them, and moving them
out would give Rust and JS callers different verdicts.

### 4.2 The seam

Whichever option is chosen, concerto-wasm needs more from core than a native
caller does. It needs the arena handles (`DeclId::from_index`,
`generation()`), the view constructors (`process`, `Processed*`,
`validate_detached_*`), the collaborator fallback (`ResolutionContext`,
`ValidatedElement`, `Node`), the TS-class detail of every error (the eight
kinds, `component`, `model_file`, `pre_port`), and an instance validator
that accepts JS values that JSON cannot carry (the `$$`-tagged `Value`). This
is the **seam** of 3.3. None of its signatures mentions a JS *type*, but it is
shaped for one caller, carries no stability promise, and does not belong in
`cargo public-api` output.

The seam uses crate-private internals: the arena slots, `instance::model`
(the field staging), the populator's `reject_unknown_keys` and
`reject_required_null`, `validate_instance_from`, `ecma`, and the catalogue.
So it cannot live in another crate unless those internals become public
under some name.

### 4.3 Option A: a `js-compat` feature in `concerto-core`

Everything in groups JS object model and Seam stays where it is, under
`#[cfg(feature = "js-compat")]`. concerto-wasm and core's own tests enable
the feature.

- **For:** no code moves. The diff is `cfg` attributes and visibility, so R3
  holds by construction: the WASM build compiles the same code as today.
  Internals stay `pub(crate)`. This can be done in one step.
- **Against:** the JS object model's 4,600 lines stay in the core crate, so
  the "JS-centric crate" criticism is only half answered: the API is clean,
  the crate is not. Cargo features are additive and unify across a build, so
  any crate in a graph that enables `js-compat` turns it on for everyone in
  that graph. The default surface must therefore be checked with core built
  alone (`cargo public-api -p accordproject-concerto-core`). Core then has two
  configurations to build, lint and test.

### 4.4 Option B: a separate crate, `concerto-core-js`

A new workspace member (`publish = false`, like concerto-wasm) holds the JS
object model: `JsValue`, `Instance`, `InstanceKind`, `Dayjs`, `UtcOffset`,
`Serializer` and `SerializerOptions`, the factory, populator, generator,
`Resource` and `ResourceId`, the `$$` encoding, and the TS exception-class
mapping. concerto-wasm depends on it.

- **For:** a compiler-enforced boundary. Core's source contains no JS object
  model, and `cargo public-api` on core cannot see it, whatever features a
  graph enables. The JS layer can change freely with no semver impact on
  core.
- **Against:** section 4.2's seam still has to be reachable from the new
  crate. Without a feature, that means making the internals public, which is
  what this task is meant to prevent, or `#[doc(hidden)] pub`, which still
  compiles for every user. It is also real code motion:
  - `instance::model` (297 lines) is built on core internals and moves with
    the object model once it is rewritten over the public introspection API;
  - the `$$` handling in `validate.rs` (the 3,895-line instance validator)
    must turn into an input abstraction that the JS crate implements, or a
    seam entry point that takes tagged values;
  - `validate_ast` and the #1273 checks must stop going through the
    `Serializer` (F8), so the native path needs its own route to the same
    checks.

  Each piece has parity risk (R3), and the validator change also has a
  performance risk on the hottest path (P5-06, #227).

### 4.5 Comparison

| | A: feature | B: separate crate | A then B (recommended) |
|---|---|---|---|
| Default public API free of JS types (exit condition 3) | yes | yes | yes, from step 1 |
| JS object model out of core's source | no | yes | yes, from step 5 |
| Internals stay private | yes | only with a seam feature or `#[doc(hidden)]` | yes (the seam feature) |
| R3 risk | none: same code | real: code motion in the validator and populator | none at step 1; the later moves are small steps, each gated by the oracle |
| Build configurations of core | 2 | 1 | 2 (default, and `js-compat` holding the seam only) |
| Size | S–M | L | S–M, then L spread over tasks |

### 4.6 Recommendation (Q1)

**Both, in order: a feature first, then a crate for the object model, and the
feature keeps only the seam.**

1. **`js-compat` feature (P6, step 1).** Gate the JS object model and the
   seam, as in option A. This meets exit condition 3 without touching
   behaviour. concerto-wasm adds `features = ["js-compat"]` and changes
   nothing else. Its exported JS API stays as it is, as the comment of
   2026-09-28 asks.
2. **`concerto-core-js` crate (P6, step 5).** Move the JS object model out
   of core into the new crate, one module at a time, starting with the
   modules whose core dependencies are already public (`dayjs`, `value`,
   `resource_id`, `generator`). Each move has to keep the oracle at
   `baseline.tsv`. After the move, `js-compat` holds only the seam, which has
   no JS types, and its crate docs say it is unstable and for the binding
   only. concerto-core-js enables it.

The feature is called `js-compat`, as the maintainer's input names it. That
also marks it as JS-only, where revision 1 called it `binding`.

**Keeping R3 honest.** Every step is a move, a rename or a `cfg`. No step
changes a check or a message. Each step is merged only after these pass:

- the native oracle, full canonical corpus plus supplement (16,242
  fixtures, 0 regressions) with `CONCERTO_ORACLE_FIXTURES` set;
- the concerto-wasm fast checks (`cargo fmt --check`, wasm32 clippy with
  `-D warnings`, `cargo check`, `sh build.sh`, `npm run smoke:node`);
- in concerto, the core suite and the api-snapshot, which must stay
  byte-identical;
- the conformance job;
- one fixed-seed fuzz shard, compared case for case with the head before the
  step.

PORTING.md section 4's grep becomes: no `wasm_bindgen` or `js_sys` in core,
and no `JsValue`, `Dayjs`, `SerializerOptions`, `$$` tag or `ts_class` in
`cargo public-api -p accordproject-concerto-core` (default features).

---

## 5. The proposed stable surface (R1)

The sketches below are signatures, not code to paste. Where a current item
is kept, its current name is in brackets.

### 5.1 Crate layout

```text
concerto_core
├── ModelManager, ModelManagerBuilder         (loading, lookups, semantic validation)
├── ModelFile                                 (one namespace)
├── introspect::{Declaration, ClassDeclaration, ClassKind, EnumDeclaration,
│                ScalarDeclaration, MapDeclaration, Import, Property,
│                Decorator, DecoratorArgument, TypeReferenceArgument,
│                validators::{Validator, NumberValidator, StringValidator,
│                             CollectionSizeValidator}}
├── instance::{ValidationOptions, Diagnostic, DiagnosticCode, Severity, ValidationReport}
├── metamodel::{validate_ast, NAMESPACE}
├── model_util::{short_name, namespace_of, qualify, parse_namespace,
│                is_valid_identifier, is_primitive_type, is_system_property}
├── rootmodel::{root_model_ast, root_model, decorator_model_ast, decorator_model}
├── prelude::{Named, FullyQualified, Typed, Decorated, DeclarationKind}
└── Error, ErrorKind, Result, Location, DetailCode, Detail
```

Revision 1 proposed new `decl`, `prop`, `names` and `system` modules. They
are dropped. `main` already had `introspect`, `model_util` and `rootmodel`,
and keeping those paths avoids breaking callers for no gain (R4).

### 5.2 Loading

```rust
impl ModelManager {
    pub fn new() -> Result<Self>;                                  // unchanged from main
    pub fn builder() -> ModelManagerBuilder;
    pub fn add_model_ast(&mut self, ast: &Value, file_name: Option<&str>) -> Result<ModelFileId>;
    pub fn add_model_ast_text(&mut self, json: &str, file_name: Option<&str>) -> Result<ModelFileId>; // the typed fast path (#239)
    pub fn add_model_asts<'a>(&mut self, models: impl IntoIterator<Item = (&'a Value, Option<&'a str>)>)
        -> Result<Vec<ModelFileId>>;                               // [add_models]
    pub fn update_model_ast(&mut self, ast: &Value, file_name: Option<&str>) -> Result<ModelFileId>;
    pub fn remove_model(&mut self, namespace: &str) -> Result<()>; // [delete_model_file]
    #[deprecated] pub fn add_model(&mut self, value: &Value, file_name: Option<String>) -> Result<()>; // main's signature
}

pub struct ModelManagerBuilder { /* private */ }
impl ModelManagerBuilder {
    pub fn decorator_validation(self, opts: DecoratorValidationOptions) -> Self;
    pub fn metamodel_validation(self, on: bool) -> Self;
    pub fn allow_reserved_system_type_names(self, on: bool) -> Self; // [dangerously_allow_…]
    pub fn build(self) -> Result<ModelManager>;
}
```

- **`new()` keeps `main`'s `Result<Self>`.** Revision 1 proposed an
  infallible `new()`. That would break every `ModelManager::new()?` written
  against `main`, the conformance harness among them, for no gain to the
  caller.
- **`add_model_ast`.** The `_ast` suffix leaves `add_model` free for the CTO
  follow-up (BR-06). `add_model` stays as a deprecated alias with `main`'s
  signature for one minor release.
- **`add_model_ast_text`** exposes P5-06d's typed read, which parses the
  text straight into the typed model. It is the fast path for a caller that
  holds JSON text, and it gives the same result as `add_model_ast` by
  construction (the typed path falls back to the `Value` path for every
  error, and the differential test in CI guards that).
- **Validation stays explicit.** `add_model_ast` loads without the semantic
  pass, and `add_model_asts` validates the batch and rolls back on failure
  (P1-06).
- **`update_model_ast` and `remove_model` mutate in place.** Today they
  return a new manager, because that is how the TS rollback was ported.
- **CTO source text** (`definitions`), `update_external_models` and
  `ModelFileSource` go to the seam until their follow-ups (CTO parsing, and
  a native model download).

### 5.3 Introspection

One lookup style. Stable methods are keyed by fully qualified name and return
borrows. The ids stay public as optional cheap keys.

```rust
impl ModelManager {
    pub fn model_file(&self, namespace: &str) -> Option<&ModelFile>;        // unchanged from main
    pub fn model_files(&self) -> impl Iterator<Item = &ModelFile>;          // unchanged from main
    pub fn get_declaration(&self, fqn: &str) -> Result<&Declaration>;       // unchanged from main
    pub fn declarations(&self) -> impl Iterator<Item = &Declaration>;
    pub fn class_declarations_of_kind(&self, kind: ClassKind) -> impl Iterator<Item = &ClassDeclaration>; // [get_asset_declarations …]
    pub fn resolve_type_name(&self, in_namespace: &str, short: &str) -> Result<String>; // unchanged from main
    pub fn is_assignable_to(&self, sub: &str, sup: &str) -> Result<bool>;   // unchanged from main
    pub fn super_type(&self, fqn: &str) -> Result<Option<&ClassDeclaration>>;
    pub fn super_types(&self, fqn: &str) -> Result<impl Iterator<Item = &ClassDeclaration>>;
    pub fn subclasses(&self, fqn: &str) -> Result<impl Iterator<Item = &ClassDeclaration>>;
    pub fn assignable_types(&self, fqn: &str) -> Result<impl Iterator<Item = &ClassDeclaration>>;
    pub fn get_all_properties(&self, fqn: &str) -> Result<Vec<&Property>>;  // main's signature restored
    pub fn properties(&self, fqn: &str) -> Result<impl Iterator<Item = (&str, &Property)>>; // with the owner's fqn
    pub fn property(&self, fqn: &str, name: &str) -> Result<Option<(&str, &Property)>>;
    pub fn property_path(&self, fqn: &str, path: &str) -> Result<&Property>; // [get_nested_property]
    pub fn identifier_field(&self, fqn: &str) -> Result<Option<&str>>;      // [identifier_field_name]
    pub fn ast(&self, opts: AstOptions) -> Value;                           // [get_ast(resolve, include_concerto)]
    pub fn filter(&self, keep: impl Fn(&Declaration) -> bool) -> Result<ModelManager>;
}
```

- **Where `main` had a name, it stays** (`get_declaration`,
  `resolve_type_name`, `get_all_properties`), with `main`'s signature. This
  is the one exception to the no-`get_` rule of 5.5. Revision 1's
  `declaration(fqn)` would clash with the seam's `declaration(DeclId)`, and
  renaming `main`'s `get_declaration` gains nothing.
- **Borrows instead of clones.** The integration branch's `get_all_properties`
  returns `(String, Property)` clones. `main`'s `Vec<&Property>` comes back,
  and `properties` adds the owner without cloning.
- **`filter` takes a predicate over `&Declaration`,** as TS does. The FQN-set
  form the oracle uses goes to the seam.
- **`#[non_exhaustive]`** on `Declaration`, `Property`, `ClassKind`,
  `DecoratorArgument` and `Validator`, so that a metamodel addition is not a
  breaking change. The `ast()` accessors returning `&mm::…` stay (PORTING.md
  1.2).
- **`FullyQualified` loses its associated `Error`.** A loaded element always
  knows its name, so the method becomes `fn fully_qualified_name(&self) ->
  String`. The fallible form moves to the seam with `ResolutionContext`.

### 5.4 Semantic validation

```rust
impl ModelManager {
    pub fn validate_models(&self) -> Result<()>;                       // unchanged; the conformance entry
    pub fn validate_model_file(&self, namespace: &str) -> Result<()>;  // [validate_model_file(&ModelFile)]
}
pub mod metamodel {
    pub const NAMESPACE: &str = "concerto.metamodel@1.0.0";           // [METAMODEL_NAMESPACE]
    pub fn validate_ast(ast: &Value) -> Result<()>;                    // version check plus structural check
    pub fn validate_structure(ast: &Value) -> Result<()>;              // [validate_metamodel]
}
```

- **First error only**, as in TS. A collect-all mode for model validation is
  a new feature (Q5).
- **`validate_models` keeps its name.** Revision 1 renamed it `validate`.
  It is the conformance harness's entry point and `main`'s name, so it stays.
- **`validate_ast` must not depend on the JS object model.** Today it goes
  through `Serializer::from_json` (F8). Step 5 gives it a direct route over
  `&Value` to the same populator checks, before the `Serializer` leaves
  core.

### 5.5 Naming rules

1. **No `get_` on new getters** (C-GETTER). The names that `main` already
   had keep their spelling (5.8).
2. **Plural nouns return iterators.** A `Vec` is returned only when the
   result is computed and owned, or when `main` returned one.
3. **`is_` / `has_` for booleans. `_ast` for anything that takes or returns
   a metamodel JSON document.**
4. **TS names are kept in doc comments** (`TS: BaseModelManager.getType`), so
   that a search still finds the port.
5. **Options are structs with `Default`, and presets are associated consts**
   (`ValidationOptions::STRICT`). There are no `bool` pairs in stable
   signatures.
6. **No `Option<&str>` standing for a JS `null` argument,** and no `Value`
   standing for a JS object that is not a metamodel document.
7. **No JS or TS names in types or variants.** A TS class name appears only
   in a doc comment.

### 5.6 Errors

A single opaque error type, with accessors:

```rust
#[derive(Debug, Clone)]
pub struct Error { /* Box<ContractError> */ }

impl Error {
    pub fn kind(&self) -> ErrorKind;
    pub fn code(&self) -> &'static str;           // the catalogue key, stable
    pub fn params(&self) -> &[(&'static str, String)];
    pub fn location(&self) -> Option<&Location>;  // typed Range, not serde_json::Value
    pub fn file_name(&self) -> Option<&str>;
    pub fn details(&self) -> &[Detail];           // #1273; empty unless a strict-option rejection
}
impl std::fmt::Display for Error { /* the message */ }
impl std::error::Error for Error {}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum ErrorKind {
    IllegalModel,     // TS IllegalModelException
    TypeNotFound,     // TS TypeNotFoundException
    Validator,        // TS BaseException from Validator.reportError (BC-39 may merge it)
    Validation,       // TS ValidationException
    InvalidArgument,  // TS Error               [ErrorKind::Error]
    MalformedInput,   // TS TypeError           [ErrorKind::JsTypeError]
    RecursionLimit,   // TS RangeError          [ErrorKind::JsRangeError]
    Metamodel,        // TS MetamodelException
}

pub type Result<T, E = Error> = std::result::Result<T, E>;
```

- **Eight kinds, one per TS class, with Rust names.** Revision 1 proposed
  coarsening the kinds and keeping the full detail in the binding. That would
  put a second, JS-only kind in the seam that every error carries. With the
  same eight kinds under Rust names, core carries no JS name, and the TS class
  is a total `match` in the JS layer (`ts_class` moves there). The error-parity
  decision of 2026-09-27 makes the class, and so the kind, part of the
  verdict, so a native caller gets that detail too.
- **Why one type.** Today a caller has to match three variants of
  `ConcertoError` to find a code, and two of them have none (F6).
- **Prerequisite (BR-07).** Move the 41 remaining `TypeNotFound` and
  `IllegalModel` construction sites and matches (F6) to catalogue entries, so
  that `code()` is total. The oracle already pins their messages.
- **`Location` becomes a typed struct** over `concerto.metamodel@1.0.0.Range`.
  The seam keeps the verbatim `serde_json::Value` it hands to TS.
- **`ConcertoError` stays as a deprecated alias of `Error`** for one minor
  release. `main`'s variants (`NamespaceNotFound`, `ValidationFailed`, and
  `IllegalModel { location: Option<String> }`) are already gone on the
  integration branch and are not restored: they cannot carry a code, and
  the crate is pre-1.0 and unpublished (D9). The crate CHANGELOG records the
  change (5.8).
- **The catalogue** (`CATALOGUE`, `CatalogueEntry`, `Renderer`) becomes
  private, apart from what the seam needs for the harness's golden tests.
  `Renderer::Globalize` names where a template came from in TS. It is not
  JS behaviour, but the variant is renamed so that no TS name is left in a
  type.

### 5.7 Instance validation (#1273, #1239)

```rust
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct ValidationOptions {
    pub convert_resources_to_relationships: bool,   // ResourceValidator
    pub permit_resources_for_relationships: bool,   // ResourceValidator
    pub reject_unknown_keys: bool,                  // #1273
    pub reject_required_null: bool,                 // #1273
}
impl ValidationOptions {
    pub const STRICT: Self = /* reject_unknown_keys + reject_required_null */; // [STRICT_VALIDATE_OPTIONS]
}

impl ModelManager {
    /// First error, as TS `Resource.validate` (the #1273 checks run first).
    pub fn validate_instance(&self, instance: &Value, opts: &ValidationOptions) -> Result<()>;     // [validate_instance_or_throw]
    /// Every violation (#1239).
    pub fn check_instance(&self, instance: &Value, opts: &ValidationOptions) -> ValidationReport;  // [validate_instance → ValidationResult]
    /// Against a named type rather than the instance's own `$class`.
    pub fn validate_instance_as(&self, fqn: &str, instance: &Value, opts: &ValidationOptions) -> Result<()>;
    pub fn check_instance_as(&self, fqn: &str, instance: &Value, opts: &ValidationOptions) -> ValidationReport;
}

pub struct ValidationReport { /* Vec<Diagnostic> */ }       // [ValidationResult]
impl ValidationReport {
    pub fn is_valid(&self) -> bool;
    pub fn diagnostics(&self) -> &[Diagnostic];
    pub fn into_result(self) -> Result<(), Self>;
}
impl IntoIterator for ValidationReport { /* Diagnostic */ }

#[non_exhaustive] pub struct Diagnostic { pub pointer: String, pub code: DiagnosticCode, pub severity: Severity, pub message: String }
#[non_exhaustive] pub enum DiagnosticCode { /* the 11 P3-03 codes */ }
#[non_exhaustive] pub enum Severity { Error, Warning }
```

- **Naming (Q2).** `validate_*` returns `Result`, and `check_*` returns a
  report. `_or_throw` is a JS idiom.
- **The #1273 checks move to the validator entry (BR-08).** When
  `reject_unknown_keys` or `reject_required_null` is set, both entries run
  the same pre-walk checks that `JSONPopulator` runs today, before the
  `ResourceValidator` walk. The populator's private `reject_unknown_keys` and
  `reject_required_null` take `&JsValue` today, so this is a conversion to
  `&Value`, not only a change of visibility. The oracle's
  `Serializer.fromJSON` strict fixtures pin the order.
- **Inputs are plain JSON,** as `Serializer.toJSON` writes it: no `$$`
  tagging, and a `DateTime` is its ISO string. The tagged form, which
  carries a live JS `Resource`, `undefined`, a `Map` or a dayjs object across
  the boundary, is seam only (4.2).
- **`ClassDeclaration::validate_instance(&self, mm, fqn, …)` is dropped** from
  the stable surface. It ignores `self` and needs the manager anyway.

### 5.8 `main`'s names (R4)

**Decision proposed: restore `main`'s names as the real API, not as
deprecated shims.** They are the idiomatic Rust names, and they are what a
caller of the Git crate had. The trait-based and TS-named forms added on the
integration branch are what get deprecated, or move to the seam.

| `main` | `af207c5` | Proposal |
|---|---|---|
| `Declaration::name()`, `ClassDeclaration::name()`, `ScalarDeclaration::name()`, `MapDeclaration::name()`, `Property::name()` (inherent) | only `Named::name()`; callers need `use concerto_core::Named` | **Restore the inherent methods.** They delegate to the same code as the trait. An inherent method takes precedence over a trait method in method-call syntax, so existing code that imports the trait still compiles (at worst with an unused-import warning). |
| `Property::type_name()` (inherent) | only `Typed::type_name()` | Restore the inherent method. |
| `ClassDeclaration::decorators()`, `Property::decorators()` → `&[mm::Decorator]` | `Decorated::decorators()` → `&[Decorator]` | Restore the inherent `decorators()`, returning the richer `&[Decorator]`. That is a type change from `main`, recorded in the CHANGELOG. `Decorator` has `name()` and typed `arguments()`, so a caller loses nothing. |
| `Declaration::declaration_kind()`, `ScalarDeclaration::declaration_kind()`, `ClassKind::declaration_kind(self)` (inherent) | only `DeclarationKind::declaration_kind()`; not on `ClassKind` | Restore all three. |
| `ScalarDeclaration::scalar_type() -> &'static str` | `-> Option<&'static str>` (JS `null` for an unknown `$class`) | Restore `&'static str`. A loaded scalar always has one of the six classes, so the `None` case is a view-only answer and moves to the seam. |
| `model_util::{short_name, namespace_of, qualify}` | `get_short_name`, `get_namespace(Option<&str>) -> Result<&str>`, `get_fully_qualified_name` | **Restore `main`'s three.** `get_short_name` and `get_fully_qualified_name` become deprecated aliases. `get_namespace` (a JS `null` argument) moves to the seam. |
| `model_util::{Namespace, parse_namespace(&str)}` | `ParsedNamespace`, `parse_namespace(Option<&str>, bool)` | A stable `parse_namespace(&str) -> Result<ParsedNamespace>`. The two-argument TS form moves to the seam. `Namespace` is not restored, since `ParsedNamespace` carries more (the semver parts). |
| `ModelManager::get_all_properties -> Vec<&Property>` | `-> Vec<(String, Property)>` | Restore `main`'s signature (5.3). |
| `ModelManager::{new, add_model, model_file, model_files, get_declaration, resolve_type_name, is_assignable_to}` | the same names | Unchanged. `add_model` is deprecated in favour of `add_model_ast`, with `main`'s signature kept. |
| `ModelFile::from_json`, `ModelFile::{namespace, version, file_name, declarations, imports, local_declaration, is_system_namespace, resolve_local_type}`, `Import::{namespace, imported_names, local_names, resolve}` | the same | Unchanged. |
| `ConcertoError::{TypeNotFound, NamespaceNotFound, IllegalModel, ValidationFailed}` | `{TypeNotFound, IllegalModel, Contract}` | Not restored; see 5.6. |

The traits stay public for generic code, and are also re-exported from a new
`concerto_core::prelude`. **Deprecation policy:** a `#[deprecated(since, note)]`
alias for one minor release, then removal. P6-03's semver check produces the
CHANGELOG entries (BR-06).

---

## 6. The P5-07 plan and P5-06d (R5)

### 6.1 The Rust-crate-API-only rows (BR-01 to BR-11)

| Row | Summary | Where this note handles it |
|---|---|---|
| BR-01 | DV-001: the first malformed field is named in node key order. | Documented in the error docs of `add_model_ast` (step 3). No change. |
| BR-02 | DV-005: Integer and Long AST fields are `f64`, so a typed round trip prints `5.0`. | `concerto-metamodel`, not core's API. It is needed before `ModelFile::ast()` and the `mm::*` accessors can promise `JSON.stringify` output. A separate task, before 1.0. |
| BR-03 | JS-modelling types in the default API. | Sections 4.6 and 7, steps 1 and 5 (`js-compat`, then `concerto-core-js`). The P5-07 row says `binding` feature; the feature is now called `js-compat`. |
| BR-04 | Binding-shaped traits and the `process` family are public. | The seam, behind `js-compat` (step 1). |
| BR-05 | Two lookup styles. | 5.3 (step 4). |
| BR-06 | 44 `get_*` names, and `add_model` takes a JSON AST. | 5.5 and 5.8, with deprecation aliases (step 4). |
| BR-07 | Three error shapes; no `#[non_exhaustive]`. | 5.6 (step 3) and step 6. |
| BR-08 | #1273 options are only reachable through `SerializerOptions`. | 5.7 (step 5). |
| BR-09 | The typed decode falls back to the `Value` path for every error. | 6.2. After BC-19; not in P6 unless BC-19 has landed. |
| BR-10 | The native loader treats truthy non-string names differently from TS. | A kind (c) behaviour (4.1). Fixed as a faithful port, or made strict with BC-19. It changes no signature. |
| BR-11 | Cross-reference to BR-01 and BR-02. | – |

The JS-facing rows (BC-xx) change behaviour, not the Rust API. Four touch
this design:

- **BC-38** fixes the public error-code format. `Error::code()` is the place
  for it, so it adds no new API.
- **BC-39** may merge `ErrorKind::Validator` into `IllegalModel` and
  `Validation`. `#[non_exhaustive]` makes that removal-free for matchers
  that have a wildcard arm.
- **BC-11** turns the cyclic-inheritance `RecursionLimit` into
  `IllegalModel`. The kind stays for real recursion limits.
- **BC-19** (a strict AST shape check on load) removes most kind (c)
  emulation, and so shrinks what the native path shares with the JS quirks.

### 6.2 P5-06d (#239): typed deserialisation

- **The typed path is the native fast path.** `ModelFile::from_json_text`
  and the proposed `ModelManager::add_model_ast_text` expose it. Its
  contract (typed success implies `Value` success with the same result) is
  what lets it be a stable entry point with no second set of errors.
- **The `typed_ast` module stays private.** Its types (`TypedDeclaration`)
  are an implementation detail of the loader.
- **BR-09 (drop the `Value` fallback) waits for BC-19.** Until the loader is
  strict, the fallback is what produces TS's first error on malformed input,
  for native and JS callers alike. When BC-19 ships, the loader for both is
  the typed read, the kind (c) coercions in `ecma` lose their last caller on
  the load path, and the native API does not change.

---

## 7. Implementation plan

Each step is its own P6 task and commit, and each keeps the oracle at
`baseline.tsv` and passes section 4.6's gate. Steps 1, 2 and 6 change no
behaviour. Steps 3 to 5 change only Rust signatures, and the WASM build must
stay byte-identical in its JS behaviour.

0. **Docs (done in this revision).** The 15 `missing_docs` warnings are
   fixed, and `#![warn(missing_docs)]` is in `lib.rs`. `RUSTDOCFLAGS="-D
   missing_docs" cargo doc -p accordproject-concerto-core --no-deps` passes,
   and so does clippy with `-D warnings`, so any new undocumented public item
   now fails CI (exit condition 2).
1. **`js-compat` feature (done in this revision)** (4.6, part 1; BR-03,
   BR-04). `[features] js-compat = []` is enabled in concerto-wasm and in
   core's `[dev-dependencies]` (a self dev-dependency with the feature, for
   the oracle harness and the unit tests). A module is gated with
   `#[cfg(feature = "js-compat")] pub mod` and `#[cfg(not(feature =
   "js-compat"))] pub(crate) mod`, and a single item with the crate's
   `js_compat_pub!` macro, which writes both forms. Without the feature the
   code is still compiled, because stable code uses some of it internally
   (F8). Gated:
   - the JS object model: `instance::{dayjs, factory, generator, populator,
     resource, resource_id, serializer, value}` and their re-exports
     (`JsValue`, `Instance`, `InstanceKind`, `Dayjs`, `UtcOffset`,
     `GeneratorOptions`, `Serializer`, `SerializerOptions`, `InstanceEnv`),
     and `DeserializeOptions::serializer_options`;
   - the `$$` encoding: `instance::validate::{DAYJS_TAG, RELATIONSHIP_TAG,
     UNDEFINED_TAG, NUMBER_TAG, BIGINT_TAG, MAP_TAG, js_special_number,
     js_map, js_bigint, js_undefined, is_js_undefined,
     validate_instance_from, validate_property_value}`;
   - the TS classes: `ErrorKind::ts_class` and `Decorator::js_name`;
   - the seam: `model_manager::{ResolutionContext, ValidatedElement, Node}`,
     `ModelManager::generation`, the ids' `from_index` and `index`,
     `validate_detached_*`, `validation::{validate_map_key,
     validate_map_value}`, the `process` family of 3.3 (`Processed*`,
     `ProcessDecision`, `field::{process, to_string, scalar_to_field_ast}`,
     `property::process`, `ScalarDeclaration::{process, validate_new,
     build_standalone, validate, to_string}`, `ClassDeclaration::{
     process_decision, kinds_compatible, identifier_redeclare_conflict,
     is_kind, to_string}`, `EnumDeclaration::to_string`,
     `MapDeclaration::to_string`, `ModelFile::check_constructor_arguments`,
     `Property::check_bound_validators`, `Decorator::validate`), the
     validators' `new` and `validate`, and `model_util`'s seam row;
   - `dcs` (Q3).

   Exit condition 3 now holds: rustdoc for core with default features names
   none of the JS object model's types, no `$$` tag and no TS class mapping.
   The handle lookups (`declaration(DeclId)` and the rest) stay public, since
   stable methods return `DeclId`s (5.3), until step 4 settles one lookup
   style.
2. **`main`'s names (done in this revision)** (5.8): the inherent `name`
   (`Declaration`, `ClassDeclaration`, `EnumDeclaration`, `MapDeclaration`,
   `ScalarDeclaration`, `Property`), `Property::{type_name, decorators}`,
   `declaration_kind` (`Declaration`, `ScalarDeclaration`, and `ClassKind`
   by value), `ScalarDeclaration::scalar_type() -> &'static str` read from
   the loaded node (the TS `getType`, `None` for a `$class` that is not
   fully qualified, stays as `Typed::type_name`), `model_util::{short_name,
   namespace_of, qualify}` (`get_short_name` and `get_fully_qualified_name`
   delegate to them), and `concerto_core::prelude`. Two parts of 5.8 move
   to step 4, where the other names get their `#[deprecated]` aliases:
   deprecating `get_short_name` and `get_fully_qualified_name`, and
   `get_all_properties -> Vec<&Property>`, whose owner-carrying callers need
   step 4's `properties` first.
3. **Errors** (5.6; BR-07). **Done in this revision:** the `ErrorKind`
   renames (`Error` to `InvalidArgument`, `JsTypeError` to `MalformedInput`,
   `JsRangeError` to `RecursionLimit`, with the TS class in each variant's
   doc comment) and `ts_class` behind `js-compat`. **Still to do:** finish
   the P1-05 migration of the legacy construction sites, introduce `Error`
   and `Location`, and alias `ConcertoError`.
4. **Loading and introspection renames** (5.2, 5.3, 5.5; BR-05, BR-06), with
   `#[deprecated]` aliases. The concerto-conformance harness and
   `concerto-validate-rs` keep compiling unchanged.
5. **Instance validation and `concerto-core-js`** (5.7, 4.6 part 2; BR-08).
   Lift the #1273 checks onto `&Value`, give `validate_ast` a route that
   avoids the `Serializer`, merge the options, and add `check_*`. Then move
   the JS object model into `concerto-core-js`, one module per commit.
6. **`#[non_exhaustive]`** on the enums named in 5.3, 5.6 and 5.7, and the
   `Send + Sync` static assertion (guarantee 6).
7. **Hand over to P6-02** (the native example) **and P6-03** (`cargo
   public-api` snapshot and `cargo semver-checks` in CI, on core alone with
   default features).

---

## 8. Open questions for the maintainer

| # | Question | Recommended default |
|---|---|---|
| Q1 | R2: a `js-compat` feature, a `concerto-core-js` crate, or both? | **Both, in order** (4.6): the feature first, which meets the exit condition with no behaviour risk, then the object model moves to `concerto-core-js`, and the feature keeps only the seam. |
| Q2 | What are the two instance-validation forms called? | `validate_instance` returns `Result<()>` and `check_instance` returns `ValidationReport` (5.7). The alternative follows #1239's TS names: `validate_instance` returns the report and `validate_instance_or_throw` returns `Result`, as today. |
| Q3 | Are the decorator command sets (`dcs`) part of the D11 surface? | **Not yet.** Behind `js-compat` for now. Their signatures are TS-shaped, and D11 does not list them. |
| Q4 | R4: restore `main`'s names as the API, or as deprecated shims? | **As the API** (5.8). The TS-named and trait-only forms are what get deprecated. |
| Q5 | Should semantic model validation get a collect-all mode like #1239's? | **No, not in P6.** It has no TS reference. |
| Q6 | Is the crate renamed from `accordproject-concerto-core`? | **No.** Publishing is out of scope (D9). |
| Q7 | `ModelManager: Default` builds a manager without the system models (F10). Keep it? | **Make `default()` equal to `new()`,** loading the system models and panicking only on the vendored-model bug that `new()` reports as an error. Removing `Default` would break `main`. |
| Q8 | `ErrorKind` names for the TS `Error`, `TypeError` and `RangeError` kinds. | `InvalidArgument`, `MalformedInput` and `RecursionLimit` (5.6). |
