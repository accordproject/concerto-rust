# Using `concerto-core` natively from Rust

**Task:** P6-02 (accordproject/concerto-rust#84), plan decision D11
(accordproject/concerto-rust#29). The design this guide follows is
`docs/public-api.md` (task P6-01, #83).

This is a walkthrough of `concerto-core`'s D11 in-scope surface, used the way
a native Rust caller uses it: with no TypeScript, no WASM, and none of
`concerto-core`'s `js-compat` feature enabled. Every snippet below is real
code, lifted from the runnable example at
[`concerto-core/examples/standalone.rs`](../concerto-core/examples/standalone.rs).
Run the example itself with:

```sh
cargo run --example standalone -p accordproject-concerto-core
# or, so a CI job checks it:
cargo test --example standalone -p accordproject-concerto-core
```

## Scope

D11 makes four capabilities available without the TS wrapper (see
`docs/public-api.md` section 1):

| In scope (this guide) | Out of scope (named follow-ups) |
|---|---|
| Loading models from their JSON AST | CTO parsing (possibly through `concerto-tree-sitter`) |
| Introspection: declarations, properties, types, inheritance | Typed instance objects and JSON generation (`Serializer`, `Factory`, `Resource`) |
| Semantic validation of the loaded models | Sample generation (`InstanceGenerator`) |
| Instance validation with diagnostics: accordproject/concerto#1273 and #1239 | |

Everything below lives in `concerto-core`'s default public API, with no
Cargo feature turned on. Nothing in this guide is behind `js-compat`.

## 1. Loading a model set from JSON ASTs

`concerto-core` does not parse CTO source text — that is
`concerto-tree-sitter`'s job, out of D11's scope. A native caller instead
loads the JSON AST a CTO-to-AST parser (or a hand-built document) already
produced, shaped like `concerto.metamodel@1.0.0.Model`:

```rust
use concerto_core::ModelManager;
use serde_json::json;

let person_model_ast = json!({
    "$class": "concerto.metamodel@1.0.0.Model",
    "namespace": "org.acme.hr@1.0.0",
    "declarations": [
        {
            "$class": "concerto.metamodel@1.0.0.ConceptDeclaration",
            "name": "Person",
            "isAbstract": false,
            "identified": {
                "$class": "concerto.metamodel@1.0.0.IdentifiedBy",
                "name": "email"
            },
            "properties": [
                { "$class": "concerto.metamodel@1.0.0.StringProperty", "name": "email", "isArray": false, "isOptional": false },
                { "$class": "concerto.metamodel@1.0.0.StringProperty", "name": "name", "isArray": false, "isOptional": false },
                { "$class": "concerto.metamodel@1.0.0.IntegerProperty", "name": "age", "isArray": false, "isOptional": true }
            ]
        }
    ]
});

let mut models = ModelManager::new()?;
models.add_model_ast(&person_model_ast, Some("hr.cto"))?;
```

- `ModelManager::new()` loads the Concerto system models (the root and
  decorator models) and is otherwise empty.
- `add_model_ast` loads one file's AST — checked for structure only — and
  returns a `ModelFileId` handle. Loading two files with the same namespace
  is an error.
- To load several files whose imports may reference each other in any
  order, use `add_model_asts` (plural): it loads the whole batch, then
  validates the manager once, and rolls every file back if either step
  fails.
- `ModelManager::builder()` gives an idiomatic way to turn on
  `decorator_validation`, `metamodel_validation`, or
  `allow_reserved_system_type_names` before loading anything, in place of
  boolean setters after the fact.

## 2. Semantic validation of the loaded model set

Loading a file only checks that its own AST is well-formed. Cross-model
checks — an unresolved import, a duplicate identifier, an inconsistent
super type — run once every file that needs to see the whole set is loaded:

```rust
models.validate_models()?;
```

`validate_model_file(&ModelFile)` runs the same pass over a single file, for
a caller that wants to validate incrementally as files are added.

## 3. Introspection

Look a declaration up by its fully qualified name, and read its shape:

```rust
let person = models.get_declaration("org.acme.hr@1.0.0.Person")?;
assert_eq!(person.name(), "Person");
assert_eq!(person.declaration_kind(), "ConceptDeclaration");

let person_class = person.as_class().expect("Person is a concept");
assert_eq!(person_class.kind(), concerto_core::ClassKind::Concept);

for (owner, property) in models.properties("org.acme.hr@1.0.0.Person")? {
    println!(
        "{owner}.{}: {} (optional: {})",
        property.name(),
        property.type_name().unwrap_or("?"),
        property.is_optional(),
    );
}
```

`Declaration` is a sum type over the four declaration shapes
(`Declaration::{Class, Enum, Scalar, Map}`), rather than a class hierarchy.
`as_class`/`as_scalar`/`as_map` borrow it as one of those shapes (`None` if
it is a different one); an enum declaration is reached with
`match`/`if let Declaration::Enum(e) = person { .. }` the same way. A
caller narrows to the shape it needs and reads the fields that shape has. Beyond `properties`, `ModelManager` answers the
rest of the inheritance and lookup questions over a fully qualified name:
`super_type`, `super_types`, `subclasses`, `assignable_types`,
`is_assignable_to`, `identifier_field`, `declarations`, and
`class_declarations_of_kind` for the class-like declarations of one
`ClassKind`.

## 4. Instance validation, first error

`validate_instance` checks a plain JSON document — the shape
`Serializer.toJSON` would write, with a `DateTime` as its ISO string and a
relationship as its URI — against the models loaded in the manager, and
stops at the first problem:

```rust
use concerto_core::instance::ValidationOptions;

let ada = json!({
    "$class": "org.acme.hr@1.0.0.Person",
    "email": "ada@example.com",
    "name": "Ada Lovelace",
    "age": 36,
});
models.validate_instance(&ada, &ValidationOptions::default())?; // Ok(())

let malformed = json!({
    "$class": "org.acme.hr@1.0.0.Person",
    "email": "grace@example.com",
    "name": 42, // wrong type
    "age": 36,
});
let err = models
    .validate_instance(&malformed, &ValidationOptions::default())
    .unwrap_err();
assert_eq!(err.kind(), concerto_core::ErrorKind::Validation);
```

`validate_instance_as(fqn, ..)` checks against a named type rather than the
instance's own `$class`, useful when the caller already knows what it
expects and wants a mismatched `$class` reported as a validation failure
rather than trusting the document.

`Error::kind()` gives one of a small, `#[non_exhaustive]` set of
`ErrorKind`s to match on (`Validation`, `TypeNotFound`, `InvalidArgument`,
`IllegalModel`, `Metamodel`, `MalformedInput`, `RecursionLimit`,
`Validator`); `Error::code()` gives the stable catalogue key underneath the
message text. Message text follows the TS reference and can change between
minor releases — match on `kind()`/`code()`, not on `Display`.

## 5. Instance validation, every diagnostic (accordproject/concerto#1239)

Where `validate_instance` stops at the first violation, `check_instance`
walks the document and reports every one it finds as a
`ValidationReport` — a `Vec<Diagnostic>` to keep working with, rather than
an `Err` to propagate with `?`:

```rust
let report = models.check_instance(&malformed, &ValidationOptions::default());
assert!(!report.is_valid());
for diagnostic in report.diagnostics() {
    println!(
        "{} [{}] {}",
        diagnostic.pointer, diagnostic.code, diagnostic.message
    );
}
```

Each `Diagnostic` carries:

- `pointer`: a JSON Pointer (RFC 6901) from the root of the validated value
  to the offending location (`""` for the root, `"/name"` for a top-level
  field, `"/tags/0"` for an array element);
- `code`: a stable, `#[non_exhaustive]` `DiagnosticCode` to match on
  (`MissingRequiredProperty`, `UndeclaredField`, `TypeViolation`,
  `InvalidEnumValue`, `EmptyIdentifier`, `AbstractClass`, `NotAssignable`,
  `NotResource`, `NotRelationship`, `ValidatorFailure`, `TypeNotFound`);
- `severity`: currently always `Error` — the field exists for a future
  check that is worth surfacing without failing validation on its own;
- `message`: a human-readable description, for logging or display, not for
  matching on.

`check_instance_as(fqn, ..)` is `check_instance`'s counterpart to
`validate_instance_as`.

A document that fails to populate at all — an unresolvable `$class`, a
malformed `DateTime`, a #1273 rejection (below) — is reported as the
diagnostics for that one failure; `check_instance` does not partially walk
a document it cannot read as an instance of its type in the first place.

## 6. Strict options (accordproject/concerto#1273)

`ValidationOptions` is `#[non_exhaustive]` and off by default in every
field; `ValidationOptions::STRICT` turns on the two accordproject/concerto#1273
checks (`reject_unknown_keys`, `reject_required_null`), and both entry
points above take it the same way:

```rust
let with_typo = json!({
    "$class": "org.acme.hr@1.0.0.Person",
    "email": "grace@example.com",
    "name": "Grace Hopper",
    "adge": 85, // a typo for `age`
});
let err = models
    .validate_instance(&with_typo, &ValidationOptions::STRICT)
    .unwrap_err();
println!("{err}"); // "Unexpected properties for type org.acme.hr@1.0.0.Person: adge"
```

`convert_resources_to_relationships` and `permit_resources_for_relationships`
are the two `ResourceValidator` options (TS
`convertResourcesToRelationships`/`permitResourcesForRelationships`): they
loosen how a relationship-typed field may be populated, rather than
tightening validation the way the #1273 pair does.

## Checking a raw AST against the metamodel

Outside `ModelManager`, `concerto_core::metamodel::validate_ast` checks a
JSON document against `concerto.metamodel@1.0.0` itself — the version check
plus the structural check TS's `BaseModelManager.validateAst` runs — useful
for validating a document before deciding whether to load it as a model at
all:

```rust
concerto_core::metamodel::validate_ast(&person_model_ast)?;
```

## Feature-parity: native Rust vs TS-only

What a caller gets from `concerto-core` directly, against what still needs
the TypeScript `@accordproject/concerto-core` package (or a follow-up not
yet built):

| Capability | Native Rust (`concerto-core`) | TS-only / follow-up |
|---|---|---|
| Load a model set | `ModelManager::add_model_ast(s)` from a JSON AST | Parsing **CTO source text** into that AST: `concerto-cto` in TS today; a native `concerto-tree-sitter` front end is a named D11 follow-up |
| Introspection | Declarations, properties, types, inheritance, over a loaded `ModelManager` (`get_declaration`, `properties`, `super_type(s)`, `subclasses`, `assignable_types`, `is_assignable_to`, …) | — fully native |
| Semantic (model) validation | `validate_models`, `validate_model_file` | — fully native |
| Metamodel (structural AST) validation | `concerto_core::metamodel::validate_ast` | — fully native |
| Instance validation, first error | `ModelManager::validate_instance(_as)` over plain JSON | — fully native |
| Instance validation, collect-all (#1239) | `ModelManager::check_instance(_as)` → `ValidationReport`/`Diagnostic` | — fully native |
| Strict deserialize options (#1273) | `ValidationOptions::{reject_unknown_keys, reject_required_null}` | — fully native |
| Typed instance objects | — | **TS-only.** `Resource`, `Typed`, `Factory`, `Serializer`, `JSONPopulator`/`JSONGenerator` stay TS-side (plan decision D7); a native form (for example `serde`-derived structs per declaration) is a named D11 follow-up, "typed instance objects and JSON generation" |
| Sample/skeleton instance generation | — | **TS-only.** `InstanceGenerator` is a named D11 follow-up, "sample generation" |
| Decorator command sets (DCS) | Present, but behind the `js-compat` feature, not the default public API | Stable native DCS API is an open question (`docs/public-api.md` Q3), not yet scheduled |
| WASM / JS bridging | — (not applicable to a native caller) | `concerto-wasm` and `concerto-core-js`, for the TS/JS embedding, unaffected by this guide |
| `cargo public-api` / semver enforcement of the surface above | Tracked as its own task | P6-03 (accordproject/concerto-rust#85) |

The "native Rust" column above is exactly this guide's six sections: every
D11 in-scope capability (`docs/public-api.md` section 1) is reachable from
`concerto-core`'s default public API today, with no `js-compat` feature and
no TypeScript involved. The "TS-only / follow-up" column is D11's own
out-of-scope list, unchanged by this task.
