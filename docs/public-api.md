# The public API of `concerto-core` as a standalone Rust library

**Status: design for maintainer agreement, revision 4.** Task P6-01
(accordproject/concerto-rust#83), plan decision D11
(accordproject/concerto-rust#29).

- **Revision 1** was the early draft, merged through #161 at `b211ae9`. It was
  written before P5-03 (#74) and changed no code.
- **Revision 2** redid the audit at the integration head `af207c5`, after
  P5-03 closed and P5-10d (#285) merged. It took in the maintainer's input of
  2026-09-27 on #83: the integration branch has made `concerto-core`
  JS-centric compared with `main`, and the design must isolate the
  JS-compatibility layer from the crate's public API. It also took in the
  P5-07 breaking-changes plan (concerto `migration/BREAKING-CHANGES-PLAN.md`,
  rows BR-01 to BR-11) and the P5-06d typed deserialisation (#239). It
  shipped section 7's steps 0 to 2 and the naming half of step 3.
- **Revision 3** followed the coordinator's scope of 2026-09-28 on #83
  after the review of `838ca0b`, and shipped steps 3, 4 and 6 as well:
  - **Step 3:** the opaque `Error`, `Location`, the deprecated
    `ConcertoError` alias, and the shim-only error items behind
    `js-compat`.
  - **Step 4:** the loading and introspection names of sections 5.2, 5.3
    and 5.5, with `#[deprecated]` aliases for the TS names, and the rest of
    section 5.8.
  - **Step 6:** `#[non_exhaustive]` and the `Send + Sync` assertion.

  Section 3.5 records where the code differs from the sketches of
  revision 2, and why.
- **Revision 4** is this one. It ships step 5, which the coordinator's
  comment of 2026-09-28 on #83 (5865108023) keeps in P6-01, so revision 3's
  Q9 (a follow-up task for it) is withdrawn:
  - **Step 5a:** the instance-validation API of section 5.7 over a plain
    `&Value`, with the #1273 options in `ValidationOptions`, on a native
    route of `Serializer.fromJSON` (`instance::from_json`) that the
    metamodel checks now take too (F8).
  - **Step 5b:** the JS object model moves out of core into the new,
    unpublished `concerto-core-js` crate, which concerto-wasm depends on.

  Section 3.5 and the sketch sections (4.4, 5.4 and 5.7) record where the
  code differs from revision 3's plan, and why.

concerto-wasm enables `js-compat`, and its exported JS API is unchanged: it
still reports the error kinds to JS by their old names, and it sends the TS
error factory the same payload. The comment on #83 of 2026-09-28 requires a
maintainer decision before any change to public naming or error types beyond
this note. Section 8 lists what is still open.

Every number and name in sections 3 to 6 describes `claude/tender-pascal-ocwf9q`
at `af207c5`, the code before this task, unless it says otherwise. "`main`" means concerto-rust `main` at
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
not met at `af207c5`. Section 7's step 1, done in revision 2, meets it,
revision 3 puts the rest of the seam behind the same feature, and revision
4 (step 5) moves the JS object model out of core: no type named `JsValue`
is left in `concerto-core/src`.

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
drifted onto `decorator_validation`. Revision 2 fixes all 15 and turns the
lint on (section 7, step 0). Rustdoc also reported 55 other warnings: 49
public docs that link to private items and 6 unresolved links. Step 1, done
in revision 2, fixes them: rustdoc gives no warning for core with or
without `js-compat`.

**F8. The #1273 options are only reachable through the JS object model.**
`DeserializeOptions` takes effect only inside `JSONPopulator`, through
`Serializer::from_json` with a `SerializerOptions` bag of `JsValue`s. The
native `validate_instance(&Value, &ValidateOptions)` does not take them.
`validate_metamodel` gets them by building a `Serializer` internally, so the
stable `validate_ast` depends on the JS object model today. And the native
`validate_instance` only accepts the validator's own value shape: a
`DateTime` given as its ISO string, or a relationship given as its URI
(what `Serializer.toJSON` writes), is a type violation there, because only
the populator turns them into a dayjs and a `Relationship`. Revision 4
(step 5a) fixes both: section 5.7.

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

### 3.5 Where the groups stand at revision 4

| Group (3.3) | At revision 4 |
|---|---|
| Stable | Public with default features, under the names of sections 5.2 to 5.8. The TS-named forms they replace are `#[deprecated]` aliases (5.8's policy). |
| JS object model | Moved to the `concerto-core-js` crate (step 5b): `JsValue`, `Instance`, `InstanceKind`, `Serializer`, `SerializerOptions`, the factory, populator, generator and `Resource` functions, and `DeserializeOptions`. `Dayjs`, `UtcOffset` and `ResourceId` stay in core's seam (4.4). |
| Seam | Behind `js-compat`: the handle API but the four cheap-key lookups (5.3), the collaborator traits (`ResolutionContext`, `ValidatedElement`, `FullyQualified`, `Node`), the `process` family, the option setters, the CTO and file-level loaders, `resolve_type_name_at`, `filter_by_fqn`, `parse_namespace_with`, and the TS side of the error contract (5.6). |
| Follow-up | In `concerto-core-js` with the object model (`Serializer`, `Factory`, `Resource`, `InstanceGenerator`'s JSON generator). |
| Internal | `HasValidators` and `Validate` are behind `js-compat` rather than crate-private, since the oracle harness calls `Validate`. `SemVer` and `PrereleaseIdentifier` stay public, because the stable `ParsedNamespace::Full` carries a `SemVer`. |

Where the built signatures differ from revision 2's sketches, the section
that has the sketch says so: 5.2 (non-JSON text, the builder and the
setters), 5.3 (names returned with declarations, `filter`'s predicate, `ast`
returning `Result`, `FullyQualified` in the seam, the cheap-key lookups),
5.4 (`validate_model_file`, the `metamodel` module), 5.6 (the pre-port
sites, `Location` by value, the catalogue behind the feature) and, for
revision 4, 4.4 (what stays in core's seam) and 5.7 (the #1273 checks run
as the document is read, the `_as` forms, the report of an unreadable
document).

---

## 4. Isolating the JS-compatibility layer (R2)

### 4.1 What is JS-shaped, and what only looks it

There are three different things behind "JS in core". The design treats
them differently.

| Kind | Where | Size | Treatment |
|---|---|---|---|
| **(a) The JS object model.** Values and objects that exist only in a JS caller: `undefined`, a JS `Map`, a `BigInt`, a dayjs object, a TS `Resource`, and the option bags that carry them. | `instance/value.rs` (375 lines), `dayjs.rs` (748), `serializer.rs` (811), `populator.rs` (867), `factory.rs` (645), `generator.rs` (399), `resource.rs` (178), `resource_id.rs` (579); the `$$` tags in `validate.rs` | about 4,600 lines, plus the tag handling | **Leaves core** (4.4). |
| **(b) ECMAScript semantics that *are* Concerto semantics.** Concerto defines numbers as doubles, string lengths in UTF-16 code units, regexes in the ECMAScript dialect, and the `ID_REGEX`. A Rust caller must get the same verdict as a JS caller for the same model. | `ecma.rs` (336 lines, already private `mod ecma`), `regress`, `ryu-js`, the string validator | – | **Stays in core, private.** It is implementation, not API. |
| **(c) Emulation of TS quirks on malformed input.** How TS treats a numeric name, a truthy non-string, a missing `$class` or a cyclic super type, and the TS exception class each one throws. | `ecma::{to_js_string, is_truthy, to_number}` from `introspect/*`, `validation.rs`, `model_manager.rs`; `ErrorKind::{JsTypeError, JsRangeError}` | spread | **Stays in core, private,** because it decides the verdict for both callers (guarantee 2). Its error kinds get Rust names (5.6). It shrank with BC-19 and BR-09 (P5-61, section 6.2): the model loader no longer emulates TS on a malformed node. |

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

**As built in revision 4 (step 5).** The crate is `concerto-core-js`
(`publish = false`), a workspace member that depends on core with
`js-compat`. concerto-wasm and core's oracle harness (a dev-dependency)
depend on it. It holds `JsValue`, `Instance`, `InstanceKind`, `Serializer`,
`SerializerOptions`, the factory, populator, generator and `Resource`
functions, and `DeserializeOptions` (about 3,200 lines). The plan above
changed in four places:

- **`Dayjs`, `UtcOffset` and `ResourceId` stay in core's seam.** The native
  route of `Serializer.fromJSON` (5.7) must read a `DateTime` string and a
  relationship URI exactly as the JS layer does, so the date arithmetic and
  the URI parsing are kind (b) of 4.1: Concerto semantics that give native
  and JS callers the same verdict. Neither type holds a JS value.
- **The `$$` encoding and `ts_class` stay in core's seam.** The validator
  reads the tags (the "seam entry point that takes tagged values" above:
  `validate_instance_from`), and core builds some TS-faithful messages from
  the TS class name (`introspect::decorator`). Neither is in the default API.
- **`instance::model` stays in core's seam,** unchanged, because the native
  route uses it too: it is shared, not rewritten.
- **The `Factory` checks and the #1273 rejections are shared.** The JS
  layer's `check_new_resource`, `assign_field_defaults`,
  `identifiable_field_name` and the two #1273 errors call core's
  (`instance::from_json`), so there is one copy of each check. What is
  duplicated is the populator's walk itself, once over `JsValue` in the JS
  crate and once over `serde_json::Value` in core; the oracle harness checks
  the two agree (7, step 5).

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
2. **`concerto-core-js` crate (P6, step 5; done in revision 4).** Move the
   JS object model out of core into the new crate. Each move has to keep the
   oracle at `baseline.tsv`. After the move, `js-compat` holds only the
   seam, which has no JS value type, and its docs say it is unstable and for
   the binding only. concerto-core-js enables it. The modules could not move
   one at a time in the order first planned: `serializer`, `populator`,
   `factory`, `resource` and `generator` depend on each other in a cycle, so
   they moved in one commit, then `deserialize`, then `value` (the one every
   other depends on). `dayjs` and `resource_id` stay (4.4).

The feature is called `js-compat`, as the maintainer's input names it. That
also marks it as JS-only, where revision 1 called it `binding`.

**Keeping R3 honest.** Every step but 5a is a move, a rename or a `cfg`,
and changes no check or message. Step 5a adds a second route to the
`Serializer.fromJSON` checks, which the oracle harness compares with the
first on every recorded call (7, step 5). Each step is merged only after
these pass:

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
no type named `JsValue` in `concerto-core/src`, and no `Dayjs`,
`SerializerOptions`, `$$` tag or `ts_class` in `cargo public-api -p
accordproject-concerto-core` (default features).

---

## 5. The proposed stable surface (R1)

The sketches below are signatures, not code to paste. Where a current item
is kept, its current name is in brackets. Sections 5.1 to 5.6 and 5.8 are
implemented in revision 3, and 5.7 in revision 4; the sketches show the
signatures as built, and section 3.5 lists where they differ from the
earlier revisions'.

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
├── metamodel::{validate_ast, validate_structure, NAMESPACE}
├── model_util::{short_name, namespace_of, qualify, parse_namespace,
│                is_valid_identifier, is_primitive_type, is_system_property}
├── rootmodel::{root_model_ast, root_model, decorator_model_ast, decorator_model}
├── prelude::{Named, Typed, Decorated, DeclarationKind}
└── error::{Error, ErrorKind, Result, Location, Position, DetailCode, Detail}
    (Error, ErrorKind and Result also at the crate root)
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
        -> Result<Vec<ModelFileId>>;                               // [add_models], deprecated
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
  construction: since P5-61 (BR-09) both are the one typed read, over the
  text or over a `Value`, and a test in CI checks that the two agree on
  every model AST it can find.
- **Validation stays explicit.** `add_model_ast` loads without the semantic
  pass, and `add_model_asts` validates the batch and rolls back on failure
  (P1-06).
- **`update_model_ast` and `remove_model` mutate in place.** The seam's
  `update_model_file` and `delete_model_file` return a new manager, because
  that is how the TS rollback was ported. Like `add_model_ast`,
  `update_model_ast` checks the structure only.
- **Text that is not JSON** is an `IllegalModel` error from
  `add_model_ast_text`, with the pre-port code (no TS path reads JSON text
  here).
- **The builder replaces the option setters,** which move to the seam with
  the `dangerously_…` getter. The getters `decorator_validation()` and
  `metamodel_validation()` stay.
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
    pub fn model_file_id(&self, namespace: &str) -> Option<ModelFileId>;    // the cheap keys
    pub fn declaration_id(&self, fqn: &str) -> Option<DeclId>;
    pub fn file(&self, id: ModelFileId) -> Option<&ModelFile>;
    pub fn declaration(&self, id: DeclId) -> Option<&Declaration>;
    pub fn declarations(&self) -> impl Iterator<Item = (String, &Declaration)>;
    pub fn class_declarations_of_kind(&self, kind: ClassKind)
        -> impl Iterator<Item = (String, &ClassDeclaration)>;              // [get_asset_declarations …]
    pub fn enum_declarations(&self) -> impl Iterator<Item = (String, &EnumDeclaration)>; // [get_enum_declarations]
    pub fn resolve_type_name(&self, in_namespace: &str, short: &str) -> Result<String>; // main's signature restored
    pub fn is_assignable_to(&self, sub: &str, sup: &str) -> Result<bool>;   // unchanged from main
    pub fn super_type(&self, fqn: &str) -> Result<Option<(String, &Declaration)>>;     // [get_super_type]
    pub fn super_types(&self, fqn: &str) -> Result<Vec<(String, &Declaration)>>;       // [get_all_super_type_names]
    pub fn subclasses(&self, fqn: &str) -> Result<Vec<(String, &Declaration)>>;        // [get_direct_subclasses]
    pub fn assignable_types(&self, fqn: &str) -> Result<Vec<(String, &Declaration)>>;  // [get_assignable_class_declarations]
    pub fn get_all_properties(&self, fqn: &str) -> Result<Vec<&Property>>;  // main's signature restored
    pub fn properties(&self, fqn: &str) -> Result<Vec<(String, &Property)>>; // with the owner's fqn
    pub fn own_properties(&self, fqn: &str) -> Result<&[Property]>;         // [get_own_properties]
    pub fn property(&self, fqn: &str, name: &str) -> Result<Option<(String, &Property)>>; // [get_property]
    pub fn property_path(&self, fqn: &str, path: &str) -> Result<(String, &Property)>;   // [get_nested_property]
    pub fn identifier_field(&self, fqn: &str) -> Result<Option<&str>>;      // [identifier_field_name]
    pub fn ast(&self, options: AstOptions) -> Result<Value>;                // [get_ast(resolve, include_concerto)]
    pub fn filter(&self, keep: impl Fn(&str, &Declaration) -> bool) -> Result<ModelManager>;
}

impl ModelFile {                                                            // each [get_…] kept, deprecated
    pub fn local_type(&self, name: &str) -> Option<&Declaration>;           // short or qualified name
    pub fn fully_qualified_type_name(&self, name: &str) -> Option<String>;
    pub fn imported_type(&self, name: &str) -> Result<String>;
    pub fn imported_type_names(&self) -> Vec<String>;                       // [get_imports]; `imports()` is main's
    pub fn import_uri(&self, key: &str) -> Option<&str>;
    pub fn external_imports(&self) -> HashMap<String, String>;
    pub fn asset_declaration(&self, name: &str) -> Option<&Declaration>;    // and participant, transaction, event
    pub fn class_declarations(&self) -> impl Iterator<Item = &Declaration>; // and asset, participant, transaction,
                                                                            // event, concept, enum, map, scalar
}
```

- **Where `main` had a name, it stays** (`get_declaration`,
  `resolve_type_name`, `get_all_properties`), with `main`'s signature. This
  is the one exception to the no-`get_` rule of 5.5. Revision 1's
  `declaration(fqn)` would clash with the seam's `declaration(DeclId)`, and
  renaming `main`'s `get_declaration` gains nothing.
- **Borrows instead of clones.** The integration branch's `get_all_properties`
  returns `(String, Property)` clones. `main`'s `Vec<&Property>` comes back,
  and `properties` adds the owner without cloning the property.
- **A declaration comes with its fully-qualified name.** A loaded
  `Declaration` does not know its namespace (its model file does), so every
  method that finds declarations or properties by walking the models returns
  the name alongside. Revision 2's `&ClassDeclaration` could not name an
  enum, which is a super type's subclass too, so the element is a
  `&Declaration`. The walks compute their answer, so they return a `Vec`
  (rule 2 of 5.5).
- **`filter` takes a predicate over the name and the declaration,** as TS's
  predicate over a `Declaration` that knows its name. The FQN-set form the
  oracle uses goes to the seam (`filter_by_fqn`).
- **`ast` returns `Result`,** because resolving the names can fail, as TS's
  `getAst(true)` throws.
- **`#[non_exhaustive]`** on `Declaration`, `Property`, `ClassKind`,
  `DecoratorArgument` and `Validator`, so that a metamodel addition is not a
  breaking change. The `ast()` accessors returning `&mm::…` stay (PORTING.md
  1.2).
- **`FullyQualified` moves to the seam.** Revision 2 proposed an infallible
  stable form. But no loaded element implements the trait: only the seam's
  element views do, and a loaded `Declaration` or `Property` does not know
  its namespace. So the fallible trait moves to the seam with
  `ResolutionContext`, and the prelude does not have it. The stable way to a
  name is the `(String, &Declaration)` pairs above, or
  `model_util::qualify(model_file.namespace(), declaration.name())`.
- **The handle lookups that stay stable** are `model_file_id`,
  `declaration_id`, `file` and `declaration`: the "optional cheap keys", and
  what `add_model_ast`'s `ModelFileId` is good for. The rest of the handle
  API (`property_by_id`, `declaration_ids`, `property_ids`,
  `class_declarations`, `model_file_of`, `parent_of`,
  `property_default_value`, `get_type_declaration`,
  `get_super_type_declaration`, `is_type_assignable_to`,
  `get_assignable_concrete_types`, `generation`, and the six
  `get_*_declarations` that return handles) is the seam, behind `js-compat`.

### 5.4 Semantic validation

```rust
impl ModelManager {
    pub fn validate_models(&self) -> Result<()>;                       // unchanged; the conformance entry
    pub fn validate_model_file(&self, model_file: &ModelFile) -> Result<()>; // unchanged
}
pub mod metamodel {
    pub const NAMESPACE: &str = "concerto.metamodel@1.0.0";           // [METAMODEL_NAMESPACE]
    pub fn validate_ast(ast: &Value) -> Result<()>;                    // version check plus structural check
    pub fn validate_structure(ast: &Value) -> Result<()>;              // [validate_metamodel]
}
```

- **First error only**, as in TS. A collect-all mode for model validation is
  a new feature (Q5).
- **`validate_model_file` keeps its `&ModelFile` argument.** Revision 2
  proposed a namespace. The loaded file is what `model_file(namespace)`
  returns, so the change would add a not-found error for no gain.
- **`metamodel` is a new module at the crate root.** It re-exports
  `instance::metamodel`'s check under the names above. Step 5 put
  `instance::metamodel` and its old re-exports at `instance::` in the
  seam: the TS ports `validateMetaModel` and `modelManagerFromMetaModel`
  there are for the oracle harness.
- **`validate_models` keeps its name.** Revision 1 renamed it `validate`.
  It is the conformance harness's entry point and `main`'s name, so it stays.
- **`validate_ast` does not depend on the JS object model.** It went
  through `Serializer::from_json` (F8). Step 5a gave it a direct route over
  `&Value` to the same checks (`instance::from_json`, 5.7), which
  `ModelManager::validate_ast`, `validateMetaModel` and the decorator
  command sets take too, before the `Serializer` left core.

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
#[derive(Debug, Clone, PartialEq)]
pub struct Error { /* Box<{ ContractError, which pre-port check made it }> */ }

impl Error {
    pub fn kind(&self) -> ErrorKind;
    pub fn code(&self) -> &'static str;           // the catalogue key, stable
    pub fn params(&self) -> &[(&'static str, String)];
    pub fn location(&self) -> Option<Location>;   // typed Range, not serde_json::Value
    pub fn file_name(&self) -> Option<&str>;
    pub fn details(&self) -> &[Detail];           // #1273; empty unless a strict-option rejection
}

#[non_exhaustive] pub struct Location { pub start: Position, pub end: Position, pub source: Option<String> }
#[non_exhaustive] pub struct Position { pub line: u64, pub column: u64, pub offset: u64 }
impl std::fmt::Display for Error { /* the message */ }
impl std::error::Error for Error {}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum ErrorKind {
    IllegalModel,     // TS IllegalModelException
    TypeNotFound,     // TS TypeNotFoundException
    Validator,        // TS 5.0.0 BaseException from Validator.reportError; not raised since BC-39
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
- **`code()` is total (BR-07).** Revision 2 proposed moving the 41
  `ConcertoError::{TypeNotFound, IllegalModel}` construction sites (F6) to
  catalogue entries first. Revision 3 builds them instead with two
  crate-private constructors, as the `"pre-port"` contract error that
  concerto-wasm already turned them into (a `Raw` catalogue entry, and
  `typeName` for `TypeNotFound`). `code()` is then `"pre-port"` for them, the
  binding's payload is byte for byte what it was, and `Display` keeps its
  text (`type not found: …`, `illegal model: …`), which the conformance
  harness matches on. Giving each of them a TS message and a catalogue
  entry of its own is left to the tasks that port those TS throw sites:
  under the error-parity decision of 2026-09-27 only the class is part of
  the verdict, and the class is already right.
- **`Location` is a typed struct** over `concerto.metamodel@1.0.0.Range`,
  built on demand, so `location()` returns it by value. It is `None` when
  the AST's `location` is not a well-formed `Range`. The seam keeps the
  verbatim `serde_json::Value` it hands to TS.
- **`ConcertoError` stays as a deprecated alias of `Error`** for one minor
  release. `main`'s variants (`NamespaceNotFound`, `ValidationFailed`, and
  `IllegalModel { location: Option<String> }`) are already gone on the
  integration branch and are not restored: they cannot carry a code, and
  the crate is pre-1.0 and unpublished (D9). The crate CHANGELOG records the
  change (5.8).
- **The TS side of the contract is the seam.** `ContractError` itself (with
  `pre_port`, `final_message`, `component` and `model_file`),
  `ValidatorReport`, the catalogue (`CATALOGUE`, `catalogue_entry`,
  `CatalogueEntry`, `Renderer`), `Error::contract`/`into_contract` and the
  pre-port constructors are public only with `js-compat`. Revision 2
  proposed renaming `Renderer::Globalize`; behind the feature it is not in
  the stable API, so it keeps the name that says where its templates come
  from.
- **`ValidationDetail` is `Detail`,** with the old name as a deprecated
  alias.

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
    /// First error, as TS `serializer.fromJSON(instance, {validate: true, ...})` throws it.
    pub fn validate_instance(&self, instance: &Value, opts: &ValidationOptions) -> Result<()>;     // [validate_instance_or_throw]
    /// Every violation (#1239).
    pub fn check_instance(&self, instance: &Value, opts: &ValidationOptions) -> ValidationReport;  // [validate_instance → ValidationResult]
    /// Against a named type rather than the instance's own `$class`.
    pub fn validate_instance_as(&self, fqn: &str, instance: &Value, opts: &ValidationOptions) -> Result<()>;
    pub fn check_instance_as(&self, fqn: &str, instance: &Value, opts: &ValidationOptions) -> ValidationReport;
}

pub struct ValidationReport { /* Vec<Diagnostic> */ }       // [ValidationResult], kept as a deprecated alias
impl ValidationReport {
    pub fn is_valid(&self) -> bool;
    pub fn diagnostics(&self) -> &[Diagnostic];
    pub fn into_diagnostics(self) -> Vec<Diagnostic>;
    pub fn into_result(self) -> Result<(), Self>;
}
impl IntoIterator for ValidationReport { /* Diagnostic */ }
impl<'a> IntoIterator for &'a ValidationReport { /* &Diagnostic */ }

#[non_exhaustive] pub struct Diagnostic { pub pointer: String, pub code: DiagnosticCode, pub severity: Severity, pub message: String }
#[non_exhaustive] pub enum DiagnosticCode { /* the 11 P3-03 codes */ }
#[non_exhaustive] pub enum Severity { Error, Warning }
```

- **Naming (Q2).** `validate_*` returns `Result`, and `check_*` returns a
  report. `_or_throw` is a JS idiom. This is Q2's recommended default,
  built; the maintainer can still choose #1239's TS names instead.
- **Inputs are plain JSON,** as `Serializer.toJSON` writes it: no `$$`
  tagging, a `DateTime` is its ISO string, and a relationship is its URI.
  The tagged form, which carries a live JS `Resource`, `undefined`, a `Map`
  or a dayjs object across the boundary, is seam only (4.2).
- **Both modes read the document as `Serializer.fromJSON` does** (step 5a,
  `instance::from_json`): the `Factory` checks, `JSONPopulator`'s checks and
  coercions (a `DateTime` string parsed, a URI made a relationship, a
  default assigned and validated), then the `ResourceValidator` walk. So
  `validate_instance` returns the error TS `fromJSON` with `validate: true`
  throws for the same document. The route builds the validator's value
  shape directly, without the JS object model, and the oracle harness
  replays every recorded plain-JSON `Serializer.fromJSON` call (2,072 of the
  2,080 fixtures) through it and through the JS layer's serializer, and
  fails on any difference in outcome, error (kind, code, parameters and
  details) or populated instance.
- **The #1273 checks run as the document is read (BR-08),** where
  `JSONPopulator` runs them, not in a separate pre-walk as revision 3
  proposed: that keeps their order against the populator's other checks,
  which the oracle's `Serializer.fromJSON` strict fixtures pin. They are
  lifted from `&JsValue` to `&Value` in the route above; the two error
  builders are shared with the JS layer.
- **The relationship options.** A relationship property can hold a
  resource only if the reader accepts one (`acceptResourcesForRelationships`),
  so either relationship option turns that on as well as setting the
  validator's option of the same name.
- **A document that cannot be read** (a malformed `DateTime`, an unknown
  `$class`, an empty identifier, a #1273 rejection) makes `check_instance`
  report that failure as its diagnostics: one per #1273 detail, with the
  detail's path as a JSON Pointer, or one for the error, with its code
  mapped to the closest `DiagnosticCode`. A readable document is then walked
  collect-all, as P3-03 built it.
- **The `_as` forms.** An instance with a `$class` must be of a type
  assignable to the named one (else `Validation`, or a `NotAssignable`
  diagnostic), and is then read as its own type; one with no `$class` is
  read as the named type.
- **`ClassDeclaration::validate_instance(&self, mm, fqn, …)` is dropped,**
  with `validate_instance_or_throw`. It ignored `self` and needed the
  manager anyway; `validate_instance_as` replaces it. `ValidateOptions`,
  the free `validate_instance` over the tagged shape, `DeserializeOptions`
  and `STRICT_VALIDATE_OPTIONS` leave the default API: the first two are
  the seam, the last two moved to `concerto-core-js` with the populator.

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
| `ModelManager::{new, add_model, model_file, model_files, get_declaration, resolve_type_name, is_assignable_to}` | the same names; `resolve_type_name` has a third `location` argument | Unchanged. `add_model` is deprecated in favour of `add_model_ast`, with `main`'s signature kept. `resolve_type_name` gets `main`'s two arguments back; the three-argument form is the seam's `resolve_type_name_at`. |
| `ModelFile::from_json`, `ModelFile::{namespace, version, file_name, declarations, imports, local_declaration, is_system_namespace, resolve_local_type}`, `Import::{namespace, imported_names, local_names, resolve}` | the same | Unchanged. |
| `ConcertoError::{TypeNotFound, NamespaceNotFound, IllegalModel, ValidationFailed}` | `{TypeNotFound, IllegalModel, Contract}` | Not restored; see 5.6. |

The traits stay public for generic code, and are also re-exported from a new
`concerto_core::prelude`. `Decorated`'s methods are `decorators` and
`decorator`; `get_decorators` and `get_decorator` stay as deprecated provided
methods. **Deprecation policy:** a `#[deprecated(since, note)]`
alias for one minor release, then removal. P6-03's semver check produces the
CHANGELOG entries (BR-06).

---

## 6. The P5-07 plan and P5-06d (R5)

### 6.1 The Rust-crate-API-only rows (BR-01 to BR-11)

| Row | Summary | Where this note handles it |
|---|---|---|
| BR-01 | DV-001: the first malformed field is named in node key order. | Documented in the error docs of `add_model_ast` (step 3). No change. |
| BR-02 | DV-005: Integer and Long AST fields are `f64`, so a typed round trip prints `5.0`. | `concerto-metamodel`, not core's API. It is needed before `ModelFile::ast()` and the `mm::*` accessors can promise `JSON.stringify` output. A separate task, before 1.0. |
| BR-03 | JS-modelling types in the default API. | Sections 4.6 and 7, steps 1 and 5 (`js-compat`, then `concerto-core-js`), both done. The P5-07 row says `binding` feature; the feature is now called `js-compat`. |
| BR-04 | Binding-shaped traits and the `process` family are public. | The seam, behind `js-compat` (step 1). |
| BR-05 | Two lookup styles. | 5.3 (step 4). |
| BR-06 | 44 `get_*` names, and `add_model` takes a JSON AST. | 5.5 and 5.8, with deprecation aliases (step 4). |
| BR-07 | Three error shapes; no `#[non_exhaustive]`. | 5.6 (step 3) and step 6. |
| BR-08 | #1273 options are only reachable through `SerializerOptions`. | 5.7 (step 5a, done): `ValidationOptions` over `&Value`. |
| BR-09 | The typed decode falls back to the `Value` path for every error. | 6.2. **Done in P5-61** (accordproject/concerto-rust#393), after BC-19. |
| BR-10 | The native loader treats truthy non-string names differently from TS. | A kind (c) behaviour (4.1). Made strict as a side effect of BR-09 (P5-61): the native loader's typed read requires a string name. It changes no signature. |
| BR-11 | Cross-reference to BR-01 and BR-02. | – |

The JS-facing rows (BC-xx) change behaviour, not the Rust API. Four touch
this design:

- **BC-38** fixes the public error-code format. `Error::code()` is the place
  for it, so it adds no new API.
- **BC-39** (R1, P5-53) reports validator errors as `IllegalModel` (found
  while a model loads) or `Validation` (an instance value), keeping the
  `errorType` in `ContractError::validator`. `ErrorKind::Validator` is no
  longer raised, but stays in the enum, so no matcher breaks.
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
- **BR-09 (drop the `Value` fallback) is done (P5-61, accordproject/concerto-rust#393).**
  The typed read is the only model loader, for text (`ModelFile::from_json_text`)
  and for a `Value` (`ModelFile::from_json` and the rest, which read the
  `Value` through the same seeds). It is strict: a node it cannot read is a
  `modelfile-load-unreadable` `IllegalModel` error, not a TS-style coercion.
  On the JS API, BC-19's shape check rejects such an AST first, unless the
  manager opts out (`metamodelValidation: false`, trusted input: the error's
  class and message are then unspecified, but it is an error, never a trap).
  A native caller has no shape check, and gets the strict read. The native
  API did not change. Every field is decoded strictly (`identified` and the
  three validators included; the shape check requires a node there since
  P5-61), and a key a node's generated struct does not declare is refused,
  but for the parser's `DateTimeProperty` `defaultValue`. A node whose
  `$class` is not its first key is buffered and read again. The read is not
  a full metamodel check (`typed_ast`'s module doc, "Not checked").

  Which loads are shape-checked, and which are trusted by construction:

  | Path | Check |
  |---|---|
  | JS `new ModelFile(...)`, and so `fromAst`, `addModel`, `addCTOModel`, `addModelFiles`, `updateModelFile`, the file `addModelFile` is given | The TS constructor runs `check_ast_shape` first (P5-49), unless the manager opts out. |
  | JS `DecoratorManager.decorateModels` and `extract*` results | Each result model is a `new ModelFile` in a new default `ModelManager`, so it is checked. The engine-side file staged for it (P5-27) is built by Rust from already-loaded models and validated command sets: trusted by construction. |
  | JS `modelManagerFromMetaModel`, Rust `model_manager_from_meta_model` | `check_ast_shape` per model (P5-49), after the TS constructor's argument checks. |
  | The engine-side copy of a JS `ModelFile` (`stageModelFile`, `addModelWithDefinitions`, `updateModelFile`, `modelFileValidateDetached`, `modelFileFromAst`) | The AST of a `ModelFile` the TS constructor built: checked there, or opted out. |
  | The system models, the metamodel, the DCS model (embedded ASTs) | Trusted by construction. |
  | Rust `decorate_models`/`extract_*` results, `update_external_models` | Built from already-loaded models (trusted by construction), or, for `update_external_models`' downloaded models on the native API, not checked: the strict typed read. |
  | Rust native `ModelFile::from_json*`, `ModelManager::add_model_ast*`, `load_model*`, `update_model_ast` | Not checked (BR-10): the strict typed read. |
  | A detached file (`validate_detached_model_file`) | Takes a `ModelFile` already built by one of the above. |

---

## 7. Implementation plan

Each step is its own P6 task and commit, and each keeps the oracle at
`baseline.tsv` and passes section 4.6's gate. Steps 1, 2 and 6 change no
behaviour. Steps 3 to 5 change only Rust signatures, and the WASM build must
stay byte-identical in its JS behaviour.

0. **Docs (done in revision 2).** The 15 `missing_docs` warnings are
   fixed, and `#![warn(missing_docs)]` is in `lib.rs`. `RUSTDOCFLAGS="-D
   missing_docs" cargo doc -p accordproject-concerto-core --no-deps` passes,
   and so does clippy with `-D warnings`, so any new undocumented public item
   now fails CI (exit condition 2).
1. **`js-compat` feature (done in revision 2)** (4.6, part 1; BR-03,
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

   Rustdoc for core with default features names none of the JS object
   model's types, no `$$` tag and no TS class mapping. Revision 3 adds the
   error items of step 3, the handle API of step 4, the traits
   `FullyQualified`, `HasValidators` and `Validate` (the audit table's
   "Internal" row: the oracle harness calls `Validate`, so they are the seam
   rather than crate-private) and `introspect::validators::{size, length,
   regex}_validator_from_ast` (for the binding).
2. **`main`'s names (done in revision 2)** (5.8): the inherent `name`
   (`Declaration`, `ClassDeclaration`, `EnumDeclaration`, `MapDeclaration`,
   `ScalarDeclaration`, `Property`), `Property::{type_name, decorators}`,
   `declaration_kind` (`Declaration`, `ScalarDeclaration`, and `ClassKind`
   by value), `ScalarDeclaration::scalar_type() -> &'static str` read from
   the loaded node (the TS `getType`, `None` for a `$class` that is not
   fully qualified, stays as `Typed::type_name`), `model_util::{short_name,
   namespace_of, qualify}` (`get_short_name` and `get_fully_qualified_name`
   delegate to them), and `concerto_core::prelude`. Revision 3 does the two
   parts left for step 4: `get_short_name` and `get_fully_qualified_name`
   are deprecated, and `get_all_properties` returns `Vec<&Property>`.
3. **Errors** (5.6; BR-07). **Done.** Revision 2 renamed the `ErrorKind`
   variants (`Error` to `InvalidArgument`, `JsTypeError` to `MalformedInput`,
   `JsRangeError` to `RecursionLimit`, with the TS class in each variant's
   doc comment) and put `ts_class` behind `js-compat`. Revision 3 adds the
   opaque `Error` with its accessors, `Location` and `Position`, the
   deprecated `ConcertoError` alias, `Detail` (with `ValidationDetail`
   deprecated), and puts the TS side of the contract behind `js-compat`
   (5.6). The pre-port sites keep the `"pre-port"` code (5.6).
4. **Loading and introspection renames** (5.2, 5.3, 5.5; BR-05, BR-06), with
   `#[deprecated]` aliases. **Done in revision 3,** with the handle API behind
   `js-compat` apart from the four cheap-key lookups (5.3), and the
   `ModelFile`, `model_util` and `Decorated` names of 5.5 and 5.8. The
   concerto-conformance harness keeps compiling unchanged (with a
   deprecation warning for `add_model`); `concerto-validate-rs` does not
   depend on core at its current head.
5. **Instance validation and `concerto-core-js`** (5.7, 4.6 part 2; BR-08).
   **Done in revision 4,** in four commits:
   - **5a:** `instance::from_json`, `Serializer.fromJSON` over plain JSON
     (with the `Factory` checks the JS layer now shares), the section 5.7
     API on it, and `validate_ast`, `validateMetaModel`,
     `ModelManager::validate_ast` and the decorator command sets moved onto
     it (F8). The oracle harness replays every recorded plain-JSON
     `Serializer.fromJSON` call through both routes and fails on any
     difference: 2,072 calls, 1,669 accepted and 403 rejected, all the same.
   - **5b:** the new `concerto-core-js` crate takes `serializer`,
     `populator`, `factory`, `resource` and `generator` (one commit, because
     they form a dependency cycle), then `deserialize`, then `value`
     (4.4, 4.6). Each commit is a move plus path changes; the oracle stays at
     `baseline.tsv` after each.

   The native route is not slower on the P5-06 hot path: over the 77 model
   ASTs of `migration/bench/fixtures/model-sets` (20 rounds, release build,
   strict options), the metamodel check took 0.57 to 0.65 s on the native
   route against 0.89 to 0.91 s through the JS layer's serializer, with the
   same verdicts. (It skips building the JS objects and converting them to
   the validator's value shape.)

   The oracle (16,242 fixtures: 14,132 pass, 2,110 unsupported, 0
   regressions), the concerto-wasm fast checks, concerto's core suite
   through the rebuilt module and the concerto-conformance Rust harness pass
   on the result, and the concerto-wasm exported JS API is unchanged. A
   fixed-seed fuzz shard (`migration/fuzz/bin/fuzz.js --count 20000
   --run-seed 42`) gives the same outcome for every case with the module
   built from `cc41902` (before step 5) and from the result: 18,979 agree,
   890 expected and 131 unresolved divergences, byte-identical lists.
6. **`#[non_exhaustive]`** on the enums named in 5.3, 5.6 and 5.7, and the
   `Send + Sync` static assertion (guarantee 6). **Done in revision 3,** as a
   compile-time assertion in `lib.rs` for `ModelManager`, `ModelFile` and
   `Error`. `AstOptions` is a plain options struct without
   `#[non_exhaustive]`, so a caller can write it as a literal.
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
| Q9 | *Withdrawn in revision 4.* Does step 5 (instance validation, 5.7, and the `concerto-core-js` crate, 4.6 part 2) stay in P6-01, or become its own task? The coordinator's comment 5865108023 on #83 keeps it in P6-01, and revision 4 ships it (section 7). The text below is revision 3's, for the record. | **Its own task, after P6-01 merges.** It is the only step that changes behaviour or moves code: it re-ports `JSONPopulator`'s #1273 checks from `&JsValue` to `&Value` (each rejection's path and order are pinned by the `Serializer.fromJSON` strict fixtures), gives `validate_ast` a route that avoids the `Serializer` on the P5-06 hot path, and moves about 4,600 lines into a new crate. Each part needs its own oracle, fuzz and conformance gate (4.6). Its naming also rests on Q2: the recommended `validate_instance`/`check_instance` pair gives the name `ModelManager::validate_instance` a new return type, which no deprecated alias can bridge. Until then the `instance` module keeps today's items (`ValidateOptions`, `DeserializeOptions`, `ValidationResult`, `ModelManager::validate_instance(_or_throw)`, `ClassDeclaration::validate_instance(_or_throw)`), none of which names a JS type. |
