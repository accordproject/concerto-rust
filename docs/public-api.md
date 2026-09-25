# The public API of `concerto-core` as a standalone Rust library

**Status: draft, to be revisited after P5-03 (accordproject/concerto-rust#74).**
Task P6-01 (accordproject/concerto-rust#83), plan decision D11
(accordproject/concerto-rust#29).

This note is the design half of P6-01, written early with the coordinator's
approval. It changes no code. Plan decision D11 schedules the API work itself
for after the TS migration, and P5-02 (#73) will first remove the TS logic and
the engine flag that the current surface still serves. Every number and name
below describes `claude/tender-pascal-ocwf9q` at `b211ae9`. The implementation
section (7) says what has to be re-checked before any of it is applied.

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
treats them as outside the stable surface for now (open question Q3).

The related tasks are:

- P6-02 (#84): the native example and the docs;
- P6-03 (#85): `cargo public-api` and semver checks in CI;
- PORTING.md section 4: the guard that keeps WASM and JS-facing types out of
  core's public API.

---

## 2. What the stable surface promises

These are the guarantees the stable items (section 5) will carry once
section 7 is done. Items outside the stable surface carry none of them.

1. **Semver.** A breaking change to a stable item needs a minor-version bump
   while the crate is `0.x`, and a major bump after `1.0`. P6-03 enforces this
   with `cargo public-api` and `cargo semver-checks`, run with default
   features.
2. **Same verdict as the TS reference.** Loading, semantic validation and
   instance validation give the verdict that `@accordproject/concerto-core@5.0.0`
   gives (D10). For the first-error entry points, that includes the same
   first error, the same message text, byte for byte, and the same location.
   The oracle corpus is the evidence. Recorded divergences are listed in
   `DIVERGENCES.md` and are part of the contract.
3. **Stable error codes, not stable wording.** The catalogue key
   (`ContractError::code`), the #1273 `DetailCode` and the #1239
   `DiagnosticCode` are stable identifiers, safe to match on. Message text
   follows TS: if the reference changes its wording, the text changes in a
   minor release. Callers should match on codes, not messages.
4. **Deterministic order.** Iterators and `Vec`s come back in the TS order:
   files in load order, declarations and properties in AST order, and
   diagnostics in walk order (PORTING.md 3.7).
5. **No panics on input.** No malformed AST or instance makes a stable entry
   point panic. Unbounded recursion becomes an error, never a native stack
   overflow (PORTING.md 2.5).
6. **Thread safety.** `ModelManager`, `ModelFile` and `ConcertoError` are
   `Send + Sync`, and `ConcertoError` is `std::error::Error + 'static`. This
   is checked at `b211ae9` (a `fn f<T: Send + Sync>()` probe compiles). The
   guarantee is written down so that it cannot regress unnoticed: P6-03
   should add a static assertion for it.
7. **No JS in the signatures.** No stable item mentions `wasm-bindgen`,
   `js-sys` or any type that models a JS value, a JS option bag or a TS class
   name (section 4).
8. **MSRV** is the workspace `rust-version` (1.88 today). Raising it is a
   minor-version change.

---

## 3. Audit of the current public items

### 3.1 How the audit was done

- **Items:** the rustdoc `all.html` of `cargo doc -p accordproject-concerto-core --no-deps`.
- **Methods:** a count of `pub fn` outside the `*_tests.rs` files.
- **Consumers:** the `concerto_core::` paths used by `concerto-wasm/src/lib.rs`
  and by the concerto-conformance Rust harness (branch
  `claude/tender-pascal-ocwf9q-local-P0-07`). The harness uses only
  `ModelManager::new`, `add_model`, `validate_models` and the error's
  `Display`.
- **Docs:** `cargo rustc -p accordproject-concerto-core --lib -- -W missing_docs`,
  and rustdoc's own warnings.

### 3.2 Size

| Kind | Count |
|---|---|
| Public modules | 30: 8 top-level (`dcs`, `error`, `instance`, `introspect`, `model_manager`, `model_util`, `rootmodel`, `validation`), plus 2 under `dcs`, 12 under `instance` and 8 under `introspect` |
| Structs | 44 |
| Enums | 20 |
| Traits | 10 |
| Free functions | 80 |
| Type aliases | 2 (`error::Result`, `instance::SerializerOptions`) |
| Constants | 12 |
| `pub fn` (free functions and methods) | 362 |
| Re-exported proc-macro crate | `concerto_core::derive` (D5) |

About half of this is there so that the TS views can bind to it. Almost every
signature says so: its doc comment names the TS member it replaces, and many
take or return JS-shaped values.

### 3.3 Classification by module

Each item falls into one of four groups:

- **Stable:** part of the D11 surface, possibly under a new name (section 5).
- **Binding:** exists for the `concerto-wasm` views. It moves out of the
  default public API (section 6).
- **Follow-up:** belongs to an out-of-scope D11 area. It leaves the default
  public API until its follow-up designs it.
- **Internal:** used only inside the crate, or only by its tests. It becomes
  `pub(crate)`.

| Module | Items | Group | Notes |
|---|---|---|---|
| `model_manager::ModelManager`: `new`, `add_model`, `add_models`, `model_file(s)`, `get_declaration`, `get_type_declaration`, `resolve_type`, `is_assignable_to`, `derives_from`, `get_all_properties`, `get_property`, `get_own_properties`, `get_super_type(_declaration)`, `get_all_super_type_names`, `get_direct_subclasses`, `get_assignable_class_declarations`, `get_*_declarations`, `identifier_field_name`, `is_identified`, `is_system_identified`, `get_ast`, `filter` | about 35 | Stable | Renamed (5.5). |
| `ModelManager`: `add_model_with_definitions`, `update_model_file`, `delete_model_file`, `update_external_models` (and `ModelFileSource`), `resolve_meta_model`, `get_models`, `model_file_fully_qualified_type_name`, `resolve_type_name`, `get_nested_property`, `property_default_value` | 11 | Stable, second tier | Needed by a native caller that manages files. They take and return `serde_json::Value`, as TS does. They are stable once the rename lands. |
| `ModelManager`: `generation`, `file`, `declaration`, `property`, `declaration_ids`, `property_ids`, `class_declarations`, `model_file_of`, `parent_of`, `model_file_id`, `declaration_id`, `is_type_assignable_to`, `get_assignable_concrete_types` | 13 | Binding | The handle API P1-04 built for the views. `generation()` exists only so that a JS view can tell a stale snapshot, and it has no doc comment. |
| `ModelManager`: `decorator_validation`, `set_decorator_validation`, `dangerously_allow_reserved_system_type_names_in_user_models` and its setter | 4 | Stable | These become builder options (5.2). |
| `model_manager::{DeclId, ModelFileId, PropId}` | 3 | Stable type, Binding methods | PORTING.md section 4 keeps the ids in core. Their `from_index` and `index` exist for JS and move with the binding. |
| `model_manager::{ResolutionContext, ValidatedElement, Node}` | 3 | Binding | The collaborator-fallback seam (PORTING.md 1.4). Its second implementation is the JS-callback context in `concerto-wasm`. `Node::Primitive` models a JS string answer. |
| `validation`: `ModelManager::validate_models`, `validate_model_file` | 2 | Stable | `validate_models` is the conformance harness's entry point. |
| `validation`: `validate_detached_*`, `validate_map_key`, `validate_map_value` | 6 | Binding | For views built over a file or declaration not yet in the arena. PORTING.md section 4's layout calls `validation.rs` "crate-private", but it is `pub`. |
| `introspect::{Declaration, ClassDeclaration, ClassKind, EnumDeclaration, MapDeclaration, ScalarDeclaration, Property, Import, ModelFile, Decorator, DecoratorArgument, TypeReferenceArgument, WithDecorators}` and the read-only methods on them (`name`, `kind`, `is_abstract`, `super_type`, `own_properties`, `is_*`, `values`, `key_type`, `value_type`, `validator`, `default_value`, `decorators`, `arguments`, `location`, `ast`, `namespace`, `version`, `imports`, `declarations`, `get_*_declarations`, `resolve_import`, `concerto_version`) | about 110 | Stable | These are the introspection surface. `Declaration`, `Property` and `ClassKind` need `#[non_exhaustive]` (5.3). |
| `introspect` traits: `Named`, `FullyQualified`, `Typed`, `Decorated`, `DeclarationKind` | 5 | Stable | `FullyQualified` has an associated `Error` only because the JS context can fail. The native form is in 5.3. |
| `introspect` traits: `HasValidators`, `Validate` | 2 | Internal | These are load-time check plumbing. A caller validates through `ModelManager`. |
| `introspect`: `ProcessedField`, `ProcessedProperty`, `ProcessedScalar`, `ProcessDecision`, `field::process`, `property::process`, `ScalarDeclaration::process`/`validate_new`/`build_standalone`, `field::scalar_to_field_ast`, `ClassDeclaration::process_decision`/`kinds_compatible`/`identifier_redeclare_conflict`/`is_kind`/`to_string`, `EnumDeclaration::to_string`, `ModelFile::check_constructor_arguments`, `Property::check_bound_validators`, `Decorator::validate` | about 20 | Binding | These are the constructor and `process()` ports of PORTING.md 1.2. They read `serde_json::Value` ASTs that the generated types reject (TS objects without `$class`), and exist for the views' constructors. |
| `introspect::validators::{Validator, NumberValidator, StringValidator, CollectionSizeValidator}` | 4 | Stable type, Binding constructors | Reading the bounds, the regex and the compatibility checks is introspection. The `new(&dyn ValidatedElement, …)` constructors and `validate(…)` are view plumbing. |
| `introspect::DecoratorValidationOptions` | 1 | Stable | This is the `decoratorValidation` option. |
| `model_util::*` | 19 functions, 2 constants, `SemVer`, `ParsedNamespace`, `PrereleaseIdentifier` | Split | Stable: `get_namespace`, `get_short_name`, `get_fully_qualified_name`, `parse_namespace`, `is_valid_identifier`, `is_primitive_type`, `is_system_property`, `ParsedNamespace`. Binding: the ones that take a JS-shaped `Option<&str>` or answer a TS type test over a stub (`is_enum`, `is_map`, `is_scalar`, `is_valid_map_key*`, `is_valid_map_value`, `import_fully_qualified_names`, `capitalize_first_letter`, `MAP_KEY_KINDS`, `MAP_VALUE_KINDS`). Internal: `SemVer` and `PrereleaseIdentifier`, the node-semver port. |
| `rootmodel::{root_model, root_model_ast, decorator_model, decorator_model_ast}` | 4 | Stable | These are the system models. Useful for a caller that builds ASTs. |
| `error::{ConcertoError, Result, ContractError, ErrorKind, DetailCode, ValidationDetail}` | 6 | Stable, reshaped | See section 5.6. |
| `error::{ValidatorReport, Renderer, CatalogueEntry, CATALOGUE, catalogue_entry}`, `ErrorKind::ts_class`, `ContractError::pre_port`/`component`/`final_message` | 9 | Binding | The shim's exception mapping (PORTING.md 2.3) and the harness's message rendering. |
| `instance::{validate_instance, ValidateOptions, DeserializeOptions, STRICT_VALIDATE_OPTIONS, Diagnostic, DiagnosticCode, Severity, ValidationResult}`, `ModelManager::validate_instance(_or_throw)`, `ClassDeclaration::validate_instance(_or_throw)` | 11 | Stable, reshaped | Instance validation (#1273, #1239). See section 5.7. |
| `instance::{validate_metamodel, validate_ast, METAMODEL_NAMESPACE}` | 3 | Stable | `validateAst`. It takes `&serde_json::Value`, and `concerto-validate-rs` becomes a CLI over it (D3). |
| `instance::validate::{validate_instance_from, validate_property_value, DAYJS_TAG, RELATIONSHIP_TAG, UNDEFINED_TAG, NUMBER_TAG, MAP_TAG, js_map, js_special_number, js_undefined, is_js_undefined}` | 11 | Binding | These encode JS values (`undefined`, `NaN`, a `Map`, a dayjs object) inside `serde_json::Value` with `$$` tags. They are JS types in all but name. |
| `instance::value::{JsValue, Instance, InstanceKind}`, `instance::dayjs::{Dayjs, UtcOffset}`, `instance::serializer::{Serializer, SerializerOptions}`, `instance::factory::*` (including `InstanceEnv`, `NewResourceCheck`), `instance::populator::*`, `instance::generator::*`, `instance::resource::*`, `instance::resource_id::ResourceId` | about 60 | Follow-up (typed instances and JSON generation) | These are the state of the TS `Resource` objects, which D7 keeps in TS. `JsValue` models a JS value, `Dayjs` a dayjs object, `SerializerOptions` a JS option bag (`IndexMap<String, JsValue>`), and `InstanceKind::ctor` a TS class name. 14 of the crate's 15 missing-docs warnings are here. |
| `dcs::*`, `dcs::extractor::*`, `dcs::dcsconverter::*` | 25 functions, 7 types, 2 constants | Outside D11 (Q3) | These are ports of `DecoratorManager` and `DecoratorExtractor`. The signatures are the TS ones over `serde_json::Value`. |
| `derive` (the `concerto-macros` re-export) | 1 | Internal to the workspace | The derives implement core's own traits on core's own types. A downstream crate has no use for them. |

### 3.4 Findings

**F1. No `concerto-wasm` or JS *crate* type in core.** At `b211ae9` this holds:

```
cargo tree -p accordproject-concerto-core -e normal | grep -E 'wasm-bindgen|js-sys|web-sys'
```

prints nothing, and no `#[wasm_bindgen]` appears in core.

**F2. JS-*modelling* types are in core's public API, and PORTING.md section 4's
grep check fails.** The same check runs `grep -rn 'wasm_bindgen\|JsValue' concerto-core/src`.
It prints 304 lines, all of them core's own `instance::value::JsValue` enum
(declared in P3-01b), in 11 files. That enum is not `wasm_bindgen::JsValue`,
so the letter of the rule's first bullet holds. The purpose of the rule is
broken, though: "This keeps the standalone Rust interface (Phase 6, D11) from
being shaped by the shim." The same holds for the items listed next to it in
the table above: `Dayjs`, `SerializerOptions`, the `$$` tag constants,
`ErrorKind::JsTypeError`/`JsRangeError` and `ErrorKind::ts_class`. They are
all reachable from the default public API. P6-01's exit condition ("no
`concerto-wasm` or JS type appears in core's public API") is not met until
section 6 is applied.

**F3. The binding's needs set the shape of the whole surface.** Five traits
(`ResolutionContext`, `ValidatedElement`, `FullyQualified`'s associated
`Error`, `HasValidators`, `Validate`) and the `process`/`Processed*` family
exist so that a JS view can call a Rust function about an element that is not
in the arena yet. A native caller never needs them: it always has a loaded
`ModelManager`.

**F4. Two lookup styles.** The name-keyed style (`get_declaration(fqn) ->
Result<&Declaration>`) and the handle-keyed style (`declaration(DeclId) ->
Option<&Declaration>`) sit side by side. Some methods return names
(`get_all_super_type_names -> Vec<String>`) and others return handles
(`get_asset_declarations -> Vec<DeclId>`) for the same kind of question.

**F5. The naming is inconsistent.** The Rust API guidelines drop `get_` on
getters (C-GETTER), and 44 `pub fn get_*` keep the TS name, next to
getter-style names (`model_file`, `super_type`, `own_properties`) on the same
types. Some names are TS spellings (`derives_from`,
`get_assignable_class_declarations`). `add_model` loads a JSON AST, where TS
`addModel` takes CTO text. Its own `TODO` in `model_manager.rs` says so.

**F6. The error type has three overlapping shapes.** `ConcertoError::{TypeNotFound,
IllegalModel, Contract}`: the first two are the pre-P1-05 hand-built variants,
with 45 references (construction sites and matches) left in non-test files. `Contract` carries
the full `{kind, code, params, location}`. A caller cannot get a stable code
from the first two. No enum in the crate is `#[non_exhaustive]`, so adding an
error kind, a declaration kind or a diagnostic code is a breaking change.

**F7. The `missing_docs` lint gives 15 warnings.** They are:

- 10 enum variants: `JsValue` (8) and `UtcOffset` (2);
- 4 struct fields of `GeneratorOptions`;
- 1 method, `ModelManager::generation`.

All 15 are in items section 6 moves out of the default API. Rustdoc gives 43
further warnings: 37 public docs linking to private items, and 6 unresolved
links (`ModelFile::get_definitions` twice, `mm::Decorator`,
`ModelManager::identifier_field_name`, `ModelFile`,
`ClassDeclaration::is_abstract`).

**F8. The #1273 options are only reachable through the out-of-scope layer.**
`DeserializeOptions` takes effect only inside `JSONPopulator`, through
`Serializer::from_json` with a `SerializerOptions` bag of `JsValue`s. The
in-scope entry point, `validate_instance(&Value, &ValidateOptions)`, does not
take them. `validate_metamodel` reaches them by building a `Serializer`
internally. A native caller cannot ask for strict validation without using
JS-shaped types.

---

## 4. Principle: one crate, two surfaces

The binding keeps working, and nothing is duplicated. The crate gets:

- **a default public API.** This is the D11 surface of section 5, and the only
  thing `cargo public-api` sees and semver covers;
- **a binding surface behind a Cargo feature, `binding`.** It is off by
  default. `concerto-wasm` enables it, and so do core's own oracle harness and
  tests. It holds everything the Binding and Follow-up rows of 3.3 list, under
  the same paths. It comes with no stability promise, and its crate docs say
  so.

Moving the binding surface into `concerto-wasm` itself is not possible without
copying code: it calls crate-private internals (`ecma`, `instance::model`,
the arena's slots). A feature keeps it in place. It is also not a "wasm-only
`cfg`" in the sense of PORTING.md section 4: the feature is target-independent,
and the native oracle harness uses it on the host. Q1 records the alternative
(`#[doc(hidden)]`).

PORTING.md section 4's check needs one change in the same step. The grep
should look for `wasm_bindgen\|js_sys` over the default-feature build, and
check that no `JsValue`, `Dayjs` or `$$` tag is reachable from
`cargo public-api` output. Then F2 is caught mechanically.

---

## 5. The proposed stable surface

The sketches below are signatures, not code to paste. Where a current item is
kept, its current name is given in brackets.

### 5.1 Crate layout

```text
concerto_core
├── ModelManager, ModelManagerBuilder         (loading, lookups, semantic validation)
├── ModelFile                                 (one namespace)
├── decl::{Declaration, ClassDeclaration, ClassKind, EnumDeclaration,
│          ScalarDeclaration, MapDeclaration, Import}
├── prop::{Property, PropertyKind}
├── decorator::{Decorator, DecoratorArgument, TypeReference}
├── validators::{Validator, NumberValidator, StringValidator, CollectionSizeValidator}
├── instance::{ValidationOptions, Diagnostic, DiagnosticCode, Severity, ValidationReport}
├── metamodel::{validate_ast, NAMESPACE}
├── names::{namespace, short_name, qualify, parse_namespace, is_valid_identifier, is_primitive}
├── system::{root_model, decorator_model}
├── traits::{Named, FullyQualified, Typed, Decorated}
└── Error, ErrorKind, Result, Location, DetailCode, Detail
```

`introspect::` stays as a path alias until 1.0, so that current users compile
unchanged.

### 5.2 Loading

```rust
impl ModelManager {
    pub fn new() -> Self;                                          // [new() -> Result<Self>]
    pub fn builder() -> ModelManagerBuilder;
    pub fn add_model_ast(&mut self, ast: &Value, file_name: Option<&str>) -> Result<ModelFileId>; // [add_model]
    pub fn add_model_asts<'a>(&mut self, models: impl IntoIterator<Item = (&'a Value, Option<&'a str>)>)
        -> Result<Vec<ModelFileId>>;                               // [add_models]
    pub fn update_model_ast(&mut self, ast: &Value, file_name: Option<&str>) -> Result<ModelFileId>;
    pub fn remove_model(&mut self, namespace: &str) -> Result<()>; // [delete_model_file]
}

pub struct ModelManagerBuilder { /* private */ }
impl ModelManagerBuilder {
    pub fn strict(self, on: bool) -> Self;
    pub fn decorator_validation(self, opts: DecoratorValidationOptions) -> Self;
    pub fn allow_reserved_system_type_names(self, on: bool) -> Self; // [dangerously_allow_…]
    pub fn build(self) -> ModelManager;
}
```

- **`new()` becomes infallible.** It can fail today only if the vendored
  system models fail to load, which is a bug, not an input error. It becomes an
  internal `expect`, covered by a test.
- **`add_model_ast`.** The `_ast` suffix leaves `add_model` free for the CTO
  follow-up, and matches TS naming (`addModel` takes CTO text). It returns the
  file's id, as `add_models` already does.
- **Validation stays explicit.** `add_model_ast` loads without the semantic
  pass; `add_model_asts` validates the batch and rolls back on failure. That
  is today's behaviour (P1-06), and it is documented as such. `strict` is the
  TS `ModelManager` `strict` option. It is listed only if P5-02 keeps it
  (Q4).
- **`update_model_ast` and `remove_model` mutate in place.** Today they return
  a new manager, because that is how the TS rollback was ported. The rollback
  becomes internal.
- **CTO source text** (`definitions`) is only useful once CTO parsing exists.
  It stays in the binding surface until the CTO follow-up.
- `update_external_models` and `ModelFileSource` belong to the external-model
  download, which stays in JS (ledger HYBRID). They stay in the binding
  surface, and a native download API is left to a follow-up.

### 5.3 Introspection

One lookup style. Stable methods are keyed by fully qualified name and return
borrows, so there are no ids to thread through. The ids stay public, because
the arena uses them natively (PORTING.md section 4), but only as optional
cheap keys.

```rust
impl ModelManager {
    pub fn model_file(&self, namespace: &str) -> Option<&ModelFile>;
    pub fn model_files(&self) -> impl Iterator<Item = &ModelFile>;
    pub fn declaration(&self, fqn: &str) -> Result<&Declaration>;           // [get_declaration / get_type_declaration]
    pub fn declarations(&self) -> impl Iterator<Item = &Declaration>;
    pub fn class_declarations_of_kind(&self, kind: ClassKind) -> impl Iterator<Item = &ClassDeclaration>; // [get_asset_declarations …]
    pub fn resolve_type(&self, context_ns: &str, name: &str) -> Result<String>;
    pub fn super_type(&self, fqn: &str) -> Result<Option<&ClassDeclaration>>;       // [get_super_type(_declaration)]
    pub fn super_types(&self, fqn: &str) -> Result<impl Iterator<Item = &ClassDeclaration>>; // [get_all_super_type_names]
    pub fn subclasses(&self, fqn: &str) -> Result<impl Iterator<Item = &ClassDeclaration>>;  // [get_direct_subclasses]
    pub fn assignable_types(&self, fqn: &str) -> Result<impl Iterator<Item = &ClassDeclaration>>; // [get_assignable_class_declarations]
    pub fn is_assignable_to(&self, sub: &str, sup: &str) -> Result<bool>;
    pub fn properties(&self, fqn: &str) -> Result<impl Iterator<Item = (&str, &Property)>>; // [get_all_properties]; owner fqn first
    pub fn property(&self, fqn: &str, name: &str) -> Result<Option<(&str, &Property)>>;
    pub fn property_path(&self, fqn: &str, path: &str) -> Result<&Property>;         // [get_nested_property]
    pub fn identifier_field(&self, fqn: &str) -> Result<Option<&str>>;               // [identifier_field_name]
    pub fn ast(&self, opts: AstOptions) -> Value;                                    // [get_ast(resolve, include_concerto)]
    pub fn filter(&self, keep: impl Fn(&Declaration) -> bool) -> Result<ModelManager>;
}
```

- **Borrows instead of clones.** `get_all_properties` and `get_property`
  return `(String, Property)` clones today. The stable forms borrow. Where TS
  answers a list of names, the stable form answers the declarations, and
  `Named::name()` gives the name.
- **`filter` takes a predicate over `&Declaration`,** as TS does. The current
  FQN-set encoding is how the oracle replays TS predicates, and it stays in the
  binding surface.
- **Two-bool parameters become an options struct** (`AstOptions { resolve,
  include_system_namespaces }`, C-CUSTOM-TYPE).
- **The declaration and property types stay as they are:** sum types over
  `mm::*` newtypes (PORTING.md 1.1, 1.2). `Declaration`, `Property`,
  `ClassKind`, `DecoratorArgument` and `Validator` get `#[non_exhaustive]`, so
  that a metamodel addition is not a breaking change. The `ast()` accessors
  that return `&mm::…` stay; they mirror TS `.ast`, and PORTING.md 1.2 already
  allows them.
- **`FullyQualified` loses its associated `Error`.** A loaded element always
  knows its name, so the method becomes `fn fully_qualified_name(&self) ->
  String`. The fallible form moves to the binding surface with
  `ResolutionContext`.
- `Named`, `Typed`, `Decorated` and `DeclarationKind` are unchanged.
  `DeclarationKind::declaration_kind` returns the metamodel short name, and
  that is a stable string.

### 5.4 Semantic validation

```rust
impl ModelManager {
    pub fn validate(&self) -> Result<()>;                              // [validate_models]
    pub fn validate_model_file(&self, namespace: &str) -> Result<()>;  // [validate_model_file(&ModelFile)]
}
pub mod metamodel {
    pub const NAMESPACE: &str = "concerto.metamodel@1.0.0";           // [METAMODEL_NAMESPACE]
    pub fn validate_ast(ast: &Value) -> Result<()>;                    // version check + structural check
    pub fn validate_structure(ast: &Value) -> Result<()>;              // [validate_metamodel]
}
```

- **First-error only.** This matches TS, where the error, its order and its
  message are part of the verdict (guarantee 2). A collect-all mode for model
  validation is not in TS, and it would be a new feature, not a port. It is
  left out (Q5).
- **`validate_model_file` takes a namespace,** so that the caller cannot hand
  it a file that is not in the manager. Validating a file that is not in the
  manager is a binding need (`validate_detached_*`).

### 5.5 Naming rules

These apply to every stable item. They are what section 7's renames
implement.

1. **No `get_` on getters** (C-GETTER). `get_` stays only where the operation
   is a real lookup that can fail and a same-named getter would be
   misleading. There is none in the proposal.
2. **Plural nouns return iterators.** A `Vec` is returned only when the result
   is computed and owned.
3. **`is_` / `has_` for booleans. `_ast` for anything that takes or returns a
   metamodel JSON document.**
4. **TS names are kept in doc comments** (`TS: BaseModelManager.getType`), so
   that search still finds the port.
5. **Options are structs with `Default`, and presets are associated consts**
   (`ValidationOptions::STRICT`). There are no `bool` pairs in stable
   signatures.
6. **Free functions over names live in `names`.** For example,
   `model_util::get_short_name` becomes `names::short_name`. They take `&str`,
   not `Option<&str>`. The `Option` form models a JS `null` argument, and it
   stays in the binding surface.

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
    pub fn details(&self) -> &[Detail];           // #1273, empty unless a strict-option rejection
}
impl std::fmt::Display for Error { /* the TS final message, byte for byte */ }
impl std::error::Error for Error {}

#[non_exhaustive]
pub enum ErrorKind {
    IllegalModel,        // IllegalModelException
    TypeNotFound,        // TypeNotFoundException
    Validation,          // ValidationException and validator BaseException, merged
    Metamodel,           // MetamodelException
    InvalidArgument,     // plain Error / TypeError raised on bad input
    RecursionLimit,      // RangeError (PORTING.md 2.5)
}

pub type Result<T, E = Error> = std::result::Result<T, E>;
```

- **Why one type.** Today a caller has to match three variants of
  `ConcertoError` to find a code, and two of the variants have none (F6). One
  type with accessors lets `ErrorKind`, `Detail` and new fields grow without a
  breaking change.
- **Prerequisite.** Finishing P1-05's migration of the remaining `TypeNotFound`
  and `IllegalModel` construction sites (45 references, F6) to catalogue
  entries. Each needs a
  catalogue key, so that `code()` is total. The oracle already pins their
  messages, so the move can be checked.
- **`ErrorKind` stops naming JS classes.** The mapping from `ErrorKind` to TS
  class (`ts_class`), and the split of `Validator` from `Validation` and of
  `Error` from `JsTypeError`, are the shim's concern (PORTING.md 2.3). They
  stay available on the binding surface, as `ContractError::ts_kind()` with
  today's eight variants. The native kinds are a coarsening of those eight,
  and the binding keeps the full detail.
- **`Location` becomes a typed struct.** It wraps
  `concerto.metamodel@1.0.0.Range` (start and end, each with line, column and
  offset, plus the source). The binding keeps the verbatim
  `serde_json::Value` it needs to hand TS the same object.
- `ConcertoError` stays as a deprecated alias of `Error` for one minor
  release.

### 5.7 Instance validation (#1273, #1239)

Today the pieces are split across `ValidateOptions` (the `ResourceValidator`
flags), `DeserializeOptions` (the #1273 flags, reachable only through the
serializer, F8) and the P3-03 `validate_instance`/`_or_throw` pairs. The
stable surface merges them:

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
    /// First error, as TS `Resource.validate` (plus the #1273 checks first).
    pub fn validate_instance(&self, instance: &Value, opts: &ValidationOptions) -> Result<()>;        // [validate_instance_or_throw]
    /// Every violation (#1239).
    pub fn check_instance(&self, instance: &Value, opts: &ValidationOptions) -> ValidationReport;     // [validate_instance → ValidationResult]
    /// Against a named type rather than the instance's own `$class`.
    pub fn validate_instance_as(&self, fqn: &str, instance: &Value, opts: &ValidationOptions) -> Result<()>;
    pub fn check_instance_as(&self, fqn: &str, instance: &Value, opts: &ValidationOptions) -> ValidationReport;
}

pub struct ValidationReport { /* Vec<Diagnostic> */ }       // [ValidationResult]
impl ValidationReport {
    pub fn is_valid(&self) -> bool;
    pub fn diagnostics(&self) -> &[Diagnostic];
    pub fn into_result(self) -> Result<(), Self>;            // for `?`
}
impl IntoIterator for ValidationReport { /* Diagnostic */ }

#[non_exhaustive] pub struct Diagnostic { pub pointer: String, pub code: DiagnosticCode, pub severity: Severity, pub message: String }
#[non_exhaustive] pub enum DiagnosticCode { /* the 11 P3-03 codes */ }
#[non_exhaustive] pub enum Severity { Error, Warning }
```

- **Naming.** `validate_*` returns `Result`, and `check_*` returns a report. A
  `_or_throw` suffix is a JS idiom, and the #1239 proposal's own name
  `validateInstance` belongs to the collect-all form in TS. `check_*` keeps
  the Rust pair unambiguous. Q2 asks the maintainer to confirm this.
- **The #1273 checks move into the validator entry.** When
  `reject_unknown_keys` or `reject_required_null` is set, both entry points
  run the same pre-walk checks that `JSONPopulator` runs today, before the
  `ResourceValidator` walk. The code is the populator's private
  `reject_unknown_keys` and `reject_required_null` methods, lifted to take
  `&Value`. The TS order is kept, and
  the oracle's `Serializer.fromJSON` strict fixtures pin it. `ValidationDetail`
  becomes `Detail` on `Error::details()`, and `check_*` reports each detail as
  a `Diagnostic` with code `UndeclaredField` or `TypeViolation`.
- **Inputs are plain JSON.** `serde_json::Value`, as serialized by
  `Serializer.toJSON`. There is no `$$` tagging. A `DateTime` is its ISO
  string, as in JSON. The `$$`-tagged form, which carries a live JS `Resource`
  or dayjs object across the boundary, stays in the binding surface.
- **`ClassDeclaration::validate_instance(&self, mm, fqn, …)` is dropped from
  the stable surface.** It ignores `self` (the body starts `let _ = self`) and
  needs the manager anyway, so `validate_instance_as` replaces it.

### 5.8 What else stays out of the default API

These items keep working for the binding, but are not in the default API.

| Area | Why |
|---|---|
| `JsValue`, `Instance`, `InstanceKind`, `Dayjs`, `UtcOffset`, `Serializer`, `SerializerOptions`, `factory`, `populator`, `generator`, `resource`, `ResourceId` | The "typed instance objects and JSON generation" follow-up designs a native form (for example `serde` derives over generated Rust types) instead of this JS-object model. |
| `ResolutionContext`, `ValidatedElement`, `Node`, the `process`/`Processed*` family, `validate_detached_*`, the id-to-index conversions, `generation()` | The collaborator fallback and view construction (PORTING.md 1.4, 1.5). |
| `error::{CATALOGUE, catalogue_entry, CatalogueEntry, Renderer, ValidatorReport}`, `ContractError::{pre_port, component, final_message}`, the TS-class mapping | The shim's exception mapping and the harness's golden tests. |
| The `$$` tag constants and `js_*` helpers | These encode JS values in JSON. |
| `dcs` | Q3. |
| `derive` | Only core's own traits. |

---

## 6. What moves to the `binding` feature

This is the mechanical part of the implementation. `concerto-wasm` changes
only its `Cargo.toml` (`features = ["binding"]`) and the paths that section 5
renames.

1. Add `[features] binding = []` to `concerto-core/Cargo.toml`, and enable it
   in `concerto-wasm/Cargo.toml` and in core's `[dev-dependencies]` (a self
   dev-dependency with the feature, so that the oracle harness and the unit
   tests keep their access).
2. Gate each Binding and Follow-up item of 3.3 with `#[cfg(feature =
   "binding")]`, with `pub(crate)` in the `not(binding)` build where stable
   code uses it internally. For example, `validate_metamodel` still builds a
   `Serializer` internally. The pattern is:

   ```rust
   #[cfg(feature = "binding")] pub mod value;
   #[cfg(not(feature = "binding"))] pub(crate) mod value;
   ```

3. Mark the binding surface in rustdoc with `#[doc(cfg(feature = "binding"))]`
   and a crate-level note: "unstable; for `concerto-wasm` only".
4. Update PORTING.md section 4's check to the form in section 4 of this note.

---

## 7. Implementation plan, after P5-03 (#74)

Nothing here starts before #74. When it does, re-audit first, because P5-02
(#73) removes TS logic and the engine flag, and some Binding rows may lose
their last caller. Steps, each its own commit and each keeping the oracle at
`baseline.tsv`:

1. **Re-audit.** Regenerate the table in 3.3 from rustdoc, and delete binding
   items with no remaining caller in `concerto-wasm` or the tests.
2. **The `binding` feature** (section 6). After this step:
   - `cargo public-api` on default features shows no JS-modelling type (exit
     condition 3);
   - `RUSTDOCFLAGS="-D missing_docs" cargo doc -p accordproject-concerto-core --no-deps`
     passes, because the 15 missing docs in F7 all leave the default build.
     Then `#![warn(missing_docs)]` is added to `lib.rs` (exit condition 2), and
     the 43 link warnings are fixed alongside.
3. **Errors** (5.6). Finish the P1-05 migration of the legacy construction
   sites (F6). Introduce `Error`, `ErrorKind` and `Location`, and alias
   `ConcertoError`.
4. **Loading and introspection renames** (5.2, 5.3, 5.5), with
   `#[deprecated]` aliases for the old names for one minor release. Update the
   concerto-conformance harness, which uses only `new`, `add_model` and
   `validate_models`, and `concerto-validate-rs`.
5. **Instance validation** (5.7). Merge the options, lift the #1273 checks
   onto `&Value`, and add `check_*`. Add a test per #1273 scenario through
   the native entry, and keep the per-code P3-03 tests.
6. **`#[non_exhaustive]`** on the enums named in 5.3, 5.6 and 5.7, and the
   `Send + Sync` static assertion (guarantee 6).
7. **Hand over to P6-02** (the native example) **and P6-03** (`cargo
   public-api` snapshot plus `cargo semver-checks` in CI, on default
   features).

---

## 8. Open questions for the maintainer

| # | Question | Recommended default |
|---|---|---|
| Q1 | How is the binding surface hidden: a `binding` Cargo feature, or `#[doc(hidden)] pub`? | **The feature.** `#[doc(hidden)]` items still compile for every user and still count as public to semver tools unless those are configured to skip them. A feature makes the default surface exactly what `cargo public-api` reports. |
| Q2 | What are the two instance-validation forms called? | `validate_instance` returns `Result<()>` and `check_instance` returns `ValidationReport`, as in 5.7. The alternative follows #1239's TS names: `validate_instance` returns the report and `validate_instance_or_throw` returns `Result`, as today. |
| Q3 | Are the decorator command sets (`dcs`) part of the D11 surface? | **Not yet.** Keep them behind the `binding` feature. Their signatures are TS-shaped (`serde_json::Value` in and out, `bool` flags, `can_migrate`/`migrate_to` over raw JSON), and D11 does not list them. Add them to the stable surface as their own follow-up if a native user asks. |
| Q4 | Does `strict` (TS `ModelManager` option) survive P5-02? | Decide at re-audit (7.1). The builder takes whatever options P5-02 leaves. |
| Q5 | Should semantic model validation get a collect-all mode like #1239's? | **No, not in P6.** It has no TS reference, so it is a new feature, not a port. Record it as a follow-up if requested. |
| Q6 | Is the crate renamed from `accordproject-concerto-core`? | **No.** Publishing is out of scope (D9), and the lib name is already `concerto_core`. |
