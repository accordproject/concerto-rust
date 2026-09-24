# PORTING.md: rules for porting concerto-core from TypeScript to Rust

This is the rulebook for every porting task in the concerto-core migration
(plan: accordproject/concerto-rust#29; task P0-04a, #33). If you implement a
port, follow it step by step. If you review one, judge the port against it,
using the checklist in section 10. If a rule is unclear, or a real case falls
outside it, do not decide the question inside your task. Raise it on the
task's issue. The reviewer then turns it into a new rule here (plan §5:
"repeated findings become PORTING.md rules").

Sources of truth, in priority order:

1. **The oracle**: `accordproject/concerto` `migration/oracle/fixtures/**`,
   recorded from the frozen `@accordproject/concerto-core@5.0.0` (D10). If the
   oracle and anything else disagree, the oracle wins.
2. **The TS reference source**: `packages/concerto-core/src/**` on the
   `claude/tender-pascal-ocwf9q` branch of `accordproject/concerto`. Its
   compiled output is identical to the 5.0.0 npm build.
3. **The seam ledger**: `migration/ledger/SEAM_LEDGER.tsv` and `SUMMARY.md`,
   together with the maintainer's decisions on #32 (section 5 below). The ledger says
   what moves to Rust. It does not say how the moved code behaves.
4. **The plan** (#29 and `RUST_MIGRATION_PLAN.md`), for everything else.

Terms used here: a **unit** is one TS class, or one file of top-level
functions. A **member** is one ledger row. The **view** is the TS class after
conversion (P4-03 … P4-10). The **shim** is `src/engine/` in concerto-core
(P4-02).

---

## 1. Mapping TypeScript to Rust

### 1.1 Class hierarchies become sum types plus the P1-03 traits

TS uses class inheritance. Rust uses one enum per family, and the enum picks
its variant from the AST node's `$class`. `AGENTS.md` says: "prefer sum-type
over complicated traits".

| TS classes | Rust type (`concerto_core::…`) |
|---|---|
| `Declaration` (abstract), `ClassDeclaration`, `IdentifiedDeclaration`, `AssetDeclaration`, `ConceptDeclaration`, `ParticipantDeclaration`, `TransactionDeclaration`, `EventDeclaration` | `introspect::Declaration::Class(ClassDeclaration)`, where `ClassDeclaration` carries a `ClassKind` for the five kinds |
| `EnumDeclaration`, `ScalarDeclaration`, `MapDeclaration` | the other variants of `introspect::Declaration` (`ScalarDeclaration` is in `introspect::scalar`, and the map types are in `introspect::map`) |
| `Property`, `Field`, `RelationshipDeclaration`, `EnumValueDeclaration` | `introspect::Property` (the existing 9-variant enum) |
| `MapKeyType`, `MapValueType` | structs in `introspect::map` |
| `Validator`, `NumberValidator`, `StringValidator`, `CollectionSizeValidator` | `introspect::validators::Validator` enum |
| `Decorated`, `Decorator`, `DecoratorFactory` (only the RUST/HYBRID parts) | the `Decorated` trait and the `Decorator` struct in `introspect::decorator` |
| `ModelFile` | `introspect::ModelFile` |
| `BaseModelManager`, `ModelManager`, `AstModelManager`, `Introspector` | one `model_manager::ModelManager`. The three TS managers differ only in their `processFile` callback, and that stays in TS. |
| `ModelUtil` (statics) | free functions in `model_util` |
| `IllegalModelException`, `TypeNotFoundException`, `ValidationException`, `MetamodelException`, `BaseException` thrown by validators, plain `Error`, JS `TypeError` | `ErrorKind` variants (section 2). These are not types of their own. |

Mechanical rules. Apply them in order.

1. **A method defined once on a base class and inherited** becomes a method on
   the enum, or one of the P1-03 traits (`Named`, `FullyQualified`, `Decorated`,
   `HasValidators`, `Typed`, `Validate`, `DeclarationKind`) implemented for the
   enum. Use a trait only when that trait already exists (P1-03). Adding any
   other trait needs architect sign-off on the issue, under the plan's
   thresholds: more than 2 impls, or a fundamental method.
2. **A method a subclass overrides** becomes one `match` arm per overriding
   class. When the TS override calls `super.x()`, call the shared function at
   exactly the same point in the arm, so the order of checks stays the same
   (section 3.7).
3. **Kind markers** (`isEnum()`, `isClassDeclaration()`, `declarationKind()`,
   `isKey()` and so on) become `matches!` predicates or a `DeclarationKind`
   impl. The ledger classifies the constant-return TS bodies as TS: the view
   keeps them as they are, and Rust only needs them for its own logic.
4. **`instanceof X` checks** in TS logic become `match`/`matches!` on the
   variant. When the TS code distinguishes a user subclass (for example
   `DecoratorFactory`), the ledger has already made that member HYBRID or TS.
5. **Abstract `throw new Error('not implemented')` stubs and `accept()` visitor
   entry points** are not ported (ledger: TS).
6. **Collaborator getters** (`getModelFile()`, `getParent()`,
   `getModelManager()`) become ids, or calls through `ResolutionContext`
   (section 1.4). Never port a back-pointer as `Rc`/`RefCell`.
7. **TS caching fields** (`$classDeclaration`, memoised super-type lists and
   so on) are not ported unless an oracle fixture or a unit test can observe
   them.
8. **Naming.** The Rust name is the snake_case of the TS name
   (`getShortName` → `get_short_name`). The one exception is accessor methods
   on a struct, which drop `get_` (Rust API guideline C-GETTER): `getName()` →
   `name()`. Free functions keep `get_`. Every ported item carries a doc line
   `TS: <Class>.<member> (src/<file>.ts)`, so that
   `grep -rn "TS: ModelUtil.getShortName"` finds the port.

### 1.2 Newtypes over `concerto-metamodel`

`AGENTS.md`: "Types under `concerto-core` should only refer to the types from
`concerto-metamodel` using new-type pattern."

- The AST payload of every core type is the generated `mm::*` type, held
  whole (`struct X(mm::Y)`, or a struct whose only AST field is `mm::Y`,
  plus derived data such as ids). Never redeclare the fields of a metamodel
  type by hand. P1-02 deletes the hand-built structs, and a grep in review
  must find none.
- Abstract metamodel types are `$class`-tagged serde enums (P1-01). Match on
  them. Do not re-read `$class` strings from `serde_json::Value` when a
  generated enum exists.
- The public API may return `&mm::Y` from an `ast()` accessor. This mirrors
  the TS public `.ast` field. No other public signature exposes a bare `mm::*`
  type.
- Where a generated type is *less faithful* than the TS object (it collapses
  `null` into absent, or narrows a JS number to `i32`/`i64`), see Open
  decision OD-3. Do not work around it locally.

### 1.3 Where each ledger classification puts code

| Ledger | Rust (`concerto-core`) | `concerto-wasm` | TS view (P4-xx) |
|---|---|---|---|
| **RUST** | All of the member's logic, ported in the P2/P3 task named in `planned_task`, in the module named in `target_rust_module` | one binding function or method per member, plus the JS-callback `ResolutionContext` if the row has `needs_fallback=true` (1.4) | a one-line delegation, with no branches (plan §7: "keep the views branch-free"). A row with `needs_fallback=true` still delegates in one line; the collaborator-call path is kept for that member only, behind the binding (1.4). |
| **HYBRID** | Everything *except* what the ledger `reason` column says stays in JS | binding for the Rust part, plus the JS-callback `ResolutionContext` if the row has `needs_fallback=true` (1.4), or the JS regex evaluator if the reason names `options.regExp` (3.2) | the JS part named in `reason`, calling Rust for the rest. Nothing else stays in JS. |
| **TS** | nothing | nothing | unchanged. Do not port it, even if it looks easy. A TS row with `needs_fallback=true` (for example the `Factory` or `ModelManager` constructor) is not ported either: the member stays JS, so it has no Rust path to fall back from, and the flag records coupling that P2-10 lifts into fixtures (SUMMARY §10). |

- The `reason` column of a HYBRID row is a contract, unless section 5 overrides it. What stays in JS is
  exactly what it names (a user callback, the `yaml` library, dayjs object
  construction, the `processFile` parse seam, `options.regExp`, the
  `Logger`). If the port seems to need more JS than that, stop and raise it.
- If a RUST member calls a TS member, Rust carries what it needs itself. For
  example, a RUST member that formats a message uses the Rust catalogue
  (section 2), not `Globalize`.
- **Reclassifying.** Never change a classification in the TSV by hand. Edit
  `migration/ledger/classification.js`, rerun `build-ledger.js`, run
  `extract-members.js --check`, and get architect sign-off on the task issue.
  Section 5 lists the rows that #32 decisions still override because
  `classification.js` has not yet been updated for them (OD-6).
- **TSV columns.** The ledger at `accordproject/concerto` commit `c48423c` has
  `coupled_tests_grep` (the old name-based grep, kept only for comparison),
  `w_tests` (W tests that stub or spy on this member or its class),
  `direct_tests` (any test that calls the member directly) and
  `needs_fallback` (SUMMARY Method section). There is no `coupled_tests`
  column any more. Use `w_tests`/`direct_tests` for test planning and
  `needs_fallback` for the fallback decision (1.4); never use
  `coupled_tests_grep` as evidence.
- The TS logic of a converted member stays in the TS source until P5-02,
  behind the `CONCERTO_ENGINE=ts|rust` flag (P4-02). Do not delete it earlier.

### 1.4 `ResolutionContext`: collaborator calls (P1-04)

The Rust engine owns the model graph (plan §3). `ModelManager` keeps its
declarations and properties in an arena, addressed by the stable `DeclId` and
`PropId` handles defined in P1-04.

- **Every ported method that, in TS, calls a collaborator object** (a model
  file, the model manager, or a parent declaration: `this.modelFile.getType(…)`,
  `this.getModelManager().getType(…)`, `field.getParent().getModelFile()`)
  makes that call through the `ResolutionContext` trait (P1-04). It never
  reaches into `ModelManager` fields directly. Take `&impl ResolutionContext`,
  or `&dyn ResolutionContext` if the method must be object safe.
- Trait methods mirror the TS collaborator method they replace. They use the
  same name in snake_case, the same arguments, and the same failure (a
  `TypeNotFoundException` stays `ErrorKind::TypeNotFound`). Add a trait
  method only when a port needs one, and name the TS call it replaces in its
  doc line.
- The real implementation is `ModelManager` (the arena). The **fallback
  implementation** is in `concerto-wasm`. It calls back into JS objects, for
  views that a white-box test builds over sinon-stubbed parents. Wire it
  *only* for the ledger's **fallback rows** (plan §3: "the fallback is kept
  only where a W test needs it").
- **The fallback rows are exactly the TSV rows with `needs_fallback=true`**,
  whatever their classification. The coupling re-derivation (#32 point 9,
  section 5 row 9) computed that column per `it()` from P0-02's tags
  (`migration/tags/test-tags.tsv`) and the runtime sinon trace, and it is
  already in the ledger at commit `c48423c`. A row is flagged when a W test
  builds the member's class with `new` while a different class is stubbed in
  the same test, or stubs the whole class with `createStubInstance`. SUMMARY
  §9 lists them by class: **19 members across 19 classes**, all constructors
  (3 RUST: `AssetDeclaration`, `NumberValidator`, `Validator`; 7 HYBRID:
  `Declaration`, `Decorated`, `Decorator`, `Field`, `ModelFile`, `Property`,
  `StringValidator`; 9 TS). The HYBRID `reason` text does not decide this;
  the column does.
- **A RUST row with `needs_fallback=true`** stays RUST. Its view is still a
  one-line delegation with no branches in production. The only difference is
  in the binding: for that member alone, `concerto-wasm` keeps a
  collaborator-call path, so that when the view is built over a JS parent
  that is not arena-backed (a sinon-stubbed collaborator), the Rust logic
  resolves its collaborator calls through the JS-callback `ResolutionContext`
  instead of the arena. A HYBRID row with the flag is handled the same way
  for its Rust part. A TS row with the flag gets no fallback, because the
  member is not ported (1.3).
- A port that meets a W test needing the fallback on a row *without*
  `needs_fallback=true` raises it on its issue instead of wiring it; the fix
  is a ledger rebuild, not a local fallback.
- Core never knows which implementation it is talking to. Section 3.2 uses
  the same pattern for the `options.regExp` engine.

---

## 2. The error contract

Rust returns `{kind, code, params, location}` for every error (plan §3). Rust
owns the message templates. The shim maps `kind` to the TS exception class
(P4-02). About 320 TS assertions check exact message text and about 81 check
the class, so a port must match **the same verdict, message, class and
location**.

### 2.1 Shape

P1-05 defines the concrete types. Ports must use them as follows:

- `kind: ErrorKind`: selects the TS class (table 2.3).
- `code: &'static str`: the catalogue key (2.2). For `BaseException`, the
  catalogue entry also carries the TS `errorType` (OD-1).
- `params`: an ordered list of `(name, String)`. Each value is the **JS
  ToString** of what the TS code interpolates (section 3.1: numbers are
  formatted the JS way, `undefined` gives `"undefined"`, `null` gives
  `"null"`, arrays are joined with `,`, plain objects give
  `"[object Object]"`). Where TS calls `JSON.stringify(value)` first, as
  `ResourceValidator.reportFieldTypeViolation` does, the param is that JSON
  text (3.1).
- `location: Option<…>`: the AST node's `location` (`concerto.metamodel@1.0.0.Range`),
  copied **verbatim** from the node the TS code passes (`this.ast.location`,
  or the `fileLocation` argument). It is never recomputed. It is `None` exactly
  where TS passes nothing. Oracle fixtures record it as `error.location`.
- Build errors only through the P1-05 constructor or the P1-03 error-builder
  macro. Never hand-build an error string at a throw site.

### 2.2 Finding and porting each message

For each `throw` in the member you port:

1. **`Globalize.messageFormatter('<key>')` or `Globalize.formatMessage('<key>')`**:
   the template is `messages/en.json` → `en.<key>`. The catalogue key is
   `<key>`, and the text is copied byte for byte, typos and quoting included.
2. **An inline template** (`` `Class ${x} …` ``, `'…' + x + '…'`, or a literal
   string): make a catalogue entry whose text is the literal parts. Each
   `${expr}` becomes a named `{param}`, named after the expression
   (`{name}`, `{superType}`). The key follows the en.json style
   `<file stem>-<method>-<slug>`, all lower case, for example
   `classdeclaration-validate-explicitidentifierredeclared`. Record the source
   as `src/<file>.ts:<line>` in the frozen TS reference.
3. **A message produced by the JS engine** (a JS `TypeError` such as
   `Cannot read properties of undefined (reading 'decorator')`, or a RegExp
   `SyntaxError`) is still part of the behaviour when an oracle fixture or a
   unit test observes it. The corpus holds 92 such `TypeError` fixtures. Port
   it as `ErrorKind::JsTypeError` (or the matching engine kind), using the
   exact V8 message text as the template, and record it in `DIVERGENCES.md`
   as `ts-bug` (section 7.3). Stack overflow is one such case, with its own
   rule (2.5).

**Rendering is a faithful port of `globalize.ts` `messageFormatter`.** Params
are replaced in insertion order. Each `{name}` is replaced *globally*, and
replacements are applied one after another, so an inserted value that
contains `{other}` is replaced again by a later param. The replacement follows
JS `String.prototype.replace` substitution patterns (`$$`, `$&`, `` $` ``,
`$'`). Inline templates render as template literals, so no substitution
patterns apply. Every catalogue entry records which of the two renderers it
uses.

**Class decoration.** The TS exception constructors append text to the
message. `IllegalModelException` appends `' ' + "File '<name>': line L column C, to line L2 column C2. "`,
with its first character upper-cased, and the text is present even when it is
empty: the fixtures show a trailing space. The shim passes the *raw* rendered
message to the real TS constructor, which decorates it. For the native oracle
harness, Rust also provides a port of each constructor's decoration, so that
`cargo test` can compare the final message (OD-2).

### 2.3 `kind` → TS exception class

| `ErrorKind` | TS class the shim throws | `component` in the oracle | notes |
|---|---|---|---|
| `IllegalModel` | `IllegalModelException(message, modelFile, location, component)` | `@accordproject/concerto-core` | the file name for the suffix comes from the model file the TS passes |
| `TypeNotFound` | `TypeNotFoundException(typeName, message)` | `@accordproject/concerto-core` | `params` must include `typeName`. The default message is `typenotfounderror-defaultmessage`. |
| `Validation` | `ValidationException(message)` | `@accordproject/concerto-util` | the BaseException default component |
| `Metamodel` | `MetamodelException(message)` | `@accordproject/concerto-util` | |
| `Validator` | concerto-util `BaseException(message, undefined, errorType)` | `@accordproject/concerto-util` | the message is `Validator error for field \`<id>\`. <fqn>: <msg>` (`Validator.reportError`). `errorType` is `DefaultValidatorException` or `RegexValidatorException`. |
| `Error` | `Error(message)` | `null` | 638 fixtures |
| `JsTypeError` | `TypeError(message)` | `null` | reproduced engine errors (2.2 step 3); 92 fixtures |
| `JsRangeError` | `RangeError(message)` | `null` | stack overflow at a TS recursion point, message `Maximum call stack size exceeded`, location `None` (2.5); 4 fixtures |

Error classes in the corpus, for the census a P1-05 or P1-07 check can repeat
(`grep -rhoE '"error": ?\{"class": ?"[A-Za-z]+"' fixtures | sort | uniq -c`):
`Error` 638, `IllegalModelException` 606, `TypeNotFoundException` 272,
`ParseException` 225, `ValidationException` 220, `TypeError` 92,
`BaseException` 19, `RangeError` 4, `MetamodelException` 4. Every class
except `ParseException` has a kind above.

`ParseException` (225 fixtures) always comes from concerto-cto in JS, and Rust
never produces it. `SecurityException` has no throw site. There is no
`ErrorKind` type yet: today `concerto-core` has only the `ConcertoError` enum
(`concerto-core/src/error.rs`), whose variants `ConcertoError::NamespaceNotFound`
(raised in `model_manager.rs`) and `ConcertoError::ValidationFailed` (raised
in `validation.rs`) have no TS class. P1-05 replaces `ConcertoError`'s
variants with the `{kind, code, params, location}` shape, maps each use of
those two variants to the kind the TS code throws at that point (`validate()`
throws `IllegalModelException`), and deletes them. A kind that no TS class
matches must not exist.

### 2.4 Which error comes first

TS throws the **first** error it meets. Rust must return the same one:

- Run the checks in the TS source order, statement by statement, and return on
  the first failure (`?`). The collect-all APIs (P3-03 diagnostics) are
  Rust-only and must not change the first-error path.
- Use the same phase. An error TS throws in a **constructor** (`new ModelFile`,
  the validator constructors, `ClassDeclaration.process`) is a load-time error
  in Rust (`add_model` …). An error TS throws in `validate()` is a
  validate-time error. The existing two-phase split must map onto the TS one
  member by member.
- Iterate in the same order (section 3.7).

### 2.5 Unbounded TS recursion: `RangeError`, never a native stack overflow

Four fixtures record a V8 stack overflow, all from cyclic inheritance
(`concept A extends C`, `B extends A`, `C extends B`):

- `conformance/ModelManager.validateModelFiles/b25d9dd5f7e2162ef0a0b2ee`,
  `343223828152dd93cffd4c24` and `ce4c72b99abd76dea938c762`;
- `conformance/ModelManager.addCTOModel/d85b28076a8ece00c558322c`.

Each has outcome
`{"error":{"class":"RangeError","component":null,"location":null,"message":"Maximum call stack size exceeded"}}`.
There are two recursion points on this path, both in
`src/introspect/classdeclaration.ts`, and neither has a cycle check:

- `ClassDeclaration.getProperties` (line 541), which concatenates
  `classDecl.getProperties()` of its super type (line 560). It is reached
  from `ClassDeclaration.validate` ("we also have to check fields defined in
  super classes", line 270).
- `ClassDeclaration.getProperty` (line 519), which, when the name is not an
  own property, calls `classDecl.getProperty(name)` on its super type
  (line 530). It is reached from `ClassDeclaration.validate` at line 226
  (`this.getProperty(this.idField)`), which runs *before* line 270, so on a
  cyclic model where `this.idField` names no property of any declaration in
  the cycle this is the recursion that overflows first. It is also reached from
  line 583 and from every caller of `getProperty` outside `validate`.

Port neither as native recursion.

Rules:

1. **Never port such a recursion as native Rust recursion over model data.**
   A cyclic model would overflow the native stack, and the process would
   abort instead of returning an error. Walk the chain with a loop, or carry
   an explicit visited set or depth counter.
2. **When the walk meets the condition under which TS recurses forever** (a
   cycle: a declaration seen again on the same walk), return
   `ErrorKind::JsRangeError` with the V8 text `Maximum call stack size exceeded`
   verbatim (catalogue key `engine-rangeerror-maxcallstack`, no params,
   location `None`). Return it at the TS recursion point, in the same phase,
   and after exactly the checks TS runs before it gets there (2.4). Every
   earlier check still wins, for example `Could not find super type …` for a
   missing super type.
3. **Do not "improve" it** into a cycle `IllegalModelException` or any other
   message. A cycle check that TS does not have is an added check (7.1).
4. Record it in `DIVERGENCES.md` as `ts-bug` (TS lacks the cycle check, and
   the observable error is V8's), citing the four fixture ids. The first task
   that ports a recursion point on this path (P2-03 for `getProperties` and
   `getProperty`, and P2-08 for the `ModelManager` ops that reach them) adds
   the row and the golden test.
5. Deep but acyclic chains that exceed V8's stack in TS and not in Rust have
   no fixture. Rust returns the normal result, and the port records an
   `engine` row only if a fixture or test ever observes the difference.
6. A TS path that fails on a cycle without recursing has no fixture either.
   `getAllSuperTypeDeclarations` (line 502) is an iterative `for` loop that
   pushes each super type onto `results`; on a cycle it never stops pushing,
   so the array grows without bound until the process runs out of heap or V8
   throws `RangeError: Invalid array length`. Which of the two happens is not
   deterministic and no fixture records it. Do not invent an error for it,
   and do not port it as an unbounded loop that grows a `Vec` (that would
   exhaust memory in Rust too). If a port can reach it on a cycle, raise it
   on the task issue.

---

## 3. Semantics

### 3.1 JS numbers, strings and formatting

- **Every Concerto number is an IEEE-754 double**, as in JS. Integer, Long and
  Double *instance* values are `f64` in Rust. When Rust parses JSON (native
  harness, fast path), convert every number with `as_f64()` immediately.
  serde_json keeps integers above 2^53 exactly, and JS does not: `JSON.parse`
  has already rounded them. Never keep an instance value as `i64` or `u64`.
- **Integer and Long are checked the same way, as TS does**
  (`jsonpopulator.ts` `convertToObject`): the value must be a number and
  satisfy `Math.trunc(n) === n`. In Rust that is `n.trunc() == n`. There is no
  32-bit range check for Integer and no 64-bit check for Long.
  `ResourceValidator` also requires `isFinite`. Port each check exactly where
  its TS member does it. Do not merge them.
- **`-0`**: comparisons use JS `===` semantics, which Rust `f64 ==` already
  has (`-0 == 0`, `NaN != NaN`). Keep `-0` in returned values (the oracle
  encodes it as `{"@@oracle":"number","value":"-0"}`). `String(-0)` and
  `JSON.stringify(-0)` both give `"0"`.
- **Formatting numbers into text** (`${value}`, `'…' + n`, params) must follow
  ECMAScript `Number::toString`: `1` not `1.0`, `1e+21`, `0.1`, `NaN`,
  `Infinity`. Use one crate-private helper (`ryu-js`, OD-8). Never use
  `format!("{}", f64)`.
- **`JSON.stringify` in messages**: port it as a crate-private helper
  (insertion-ordered keys, no whitespace, JS number formatting, `NaN` and
  `±Infinity` become `null`). Plain `serde_json::to_string` does not match.
- **Strings are UTF-16 in JS.** `.length`, `substr`, `charAt`, `slice` and
  `indexOf` count UTF-16 code units. Use `s.encode_utf16().count()` for
  lengths (`StringValidator` min and max length). An index operation may use
  UTF-8 byte offsets only when the split point is an ASCII character, so that
  both encodings give the same substring. The port must say so in a comment
  (section 8 has an example). Default `Array.prototype.sort` compares UTF-16
  code units. Rust `str` ordering compares code points, which differs above
  U+FFFF, so compare `encode_utf16()` iterators.
- JS `toUpperCase` and Rust `to_uppercase` both use full Unicode case mapping
  (`ß` → `SS`). `charAt(0)` takes a single UTF-16 unit, and this can split a
  surrogate pair. Port it that way (`capitalizeFirstLetter`).

### 3.2 Regular expressions

- **All evaluation against a validator's regex happens in Rust** (#32
  point 7), with the `regress` crate (ECMAScript semantics). The one
  exception is a custom engine supplied through `options.regExp` (point 7a,
  below), which Rust still drives.
  P2-02 replaces the current `fancy-regex` use. This covers every place TS
  compiles or tests a validator regex: the `StringValidator` constructor
  (compile errors, and the `defaultValue` check it runs at load time),
  `validate`, `matchesRegex`, and instance validation on both Serializer
  paths.
- **`getRegex()` is for TS callers only.** The view converts the pattern and
  flags into a JS object for `StringValidator.getRegex()`, built with the
  same constructor TS picks (the next bullet). That object is never used for
  validation. Compiling the pattern twice, once in Rust and once in JS, is
  accepted.
- **The `options.regExp` custom engine** (#32 point 7a, option (b)). Only when
  a caller supplies a custom engine through `options.regExp` does evaluation
  fall back to it. That is the whole of what the ledger's HYBRID reason
  "`options.regExp`" leaves in JS (the `StringValidator` constructor,
  `validate` and `matchesRegex`).
  - **Rust is the caller.** With the Serializer single-call path (section 5
    row 6) and the load-time `defaultValue` check inside `add_model`, the
    custom engine is reached from inside Rust, so the fallback is a callback,
    not a view branch. P2-02 defines a public evaluator trait in core, with no JS types,
    in the same pattern as `ResolutionContext` (1.4). Its methods mirror the
    three things TS does with the regex object: construct it
    (`new CustomRegExp(pattern, flags)`, which may throw; the thrown message
    goes to `reportError` with `RegexValidatorException`), test a value (set
    `lastIndex = 0`, call `test`, reset `lastIndex = 0`), and render it for
    the `failed to match validation regex: ${this.regex}` message (JS
    `String(regex)`, which for a custom engine is whatever its `toString`
    returns). The default implementation is `regress`, with the
    `RegExp.prototype.toString` rendering below.
  - **The JS implementation lives in `concerto-wasm`** (section 4). It wraps
    the `options.regExp` constructor and calls it back. A model manager
    carries an optional evaluator. The binding installs the JS one only when
    `options.regExp` is present. Core never knows which engine it is using,
    and core's public API stays free of JS types. A native Rust caller (Phase
    6) may supply its own evaluator, or none.
  - **Reproduce the TS scope of the custom engine exactly**
    (`stringvalidator.ts:77-81`): TS uses `options.regExp` only when the
    validator's `field` has `getParent()`, reaching the model manager through
    `parent.getModelFile().getModelManager()`. So:
    - the validator of a `ScalarDeclaration` (built in
      `scalardeclaration.ts:105` with the declaration itself as `field`)
      **always uses the built-in engine**, `regress` in Rust, even when
      `options.regExp` is set. This includes the load-time check of the
      scalar's `defaultValue`;
    - the validator of a `Field` (`field.ts:79`) uses the custom engine when
      one is set. This includes the synthetic field that
      `Field.getScalarField()` builds with the property's parent, which
      `ResourceValidator` uses for scalar-typed properties. The scalar's
      pattern is therefore evaluated by the custom engine during instance
      validation, and by the built-in engine when the scalar declaration
      itself is constructed.
    The TS source comments on this limitation. Record it as a `ts-bug` row
    (the custom engine is not applied uniformly), with a Rust test for each
    of the two cases.
  - The length checks, their order before the regex check, and all messages
    stay in Rust whichever engine is in use.
- **Flags.** Port the effect each flag has on `matchesRegex`, which resets
  `lastIndex` to 0 and calls `test`. `g` and `d` have no effect. `y` means the
  match must start at index 0. `i`, `m`, `s`, `u` and `v` go to `regress`. With
  any other flag, the JS constructor throws `Invalid flags supplied to RegExp constructor '<flags>'`.
  The constructor catches that error and reports it through `reportError`
  with `RegexValidatorException`.
- **Without `u` or `v`, JS matches over UTF-16 code units.** Use regress's
  UTF-16 matching for those patterns. If the regress version in use lacks it,
  record an `engine` divergence.
- **`${this.regex}` in messages** is JS `RegExp.prototype.toString`:
  `/<source>/<flags>`. The source is escaped as `EscapeRegExpPattern` does
  (an empty pattern gives `(?:)`, and `/` becomes `\/`). Flags are in the
  canonical order `dgimsuvy`.
- For invalid-pattern messages, see OD-4.

### 3.3 Dates

- DateTime values are dayjs objects in TS (D7). They are created in TS, and
  `DateTimeUtil.setCurrentTime` stays TS. On the fast path Rust receives and
  returns `(epoch ms, utcOffset minutes)`. It never returns a date object.
- **Every test runs with `TZ=UTC`.** The TS `npm test` script sets it, and the
  oracle was recorded under it. Rust must never consult the system time zone
  (no `chrono::Local`). Wherever TS interprets a string as "local time", Rust
  interprets it as UTC.
- **Parsing a string must reproduce `dayjs.utc(s)`**:
  - (a) strings that do not end in `Z` go through dayjs's own `REGEX_PARSE`
    path;
  - (b) otherwise, or when that regex fails, ECMAScript `Date.parse` applies.
  - Precision is milliseconds, and extra fraction digits are truncated.
    Validity is `isValid()`.
  - The strict path first checks the exact `strictQualifiedDateTimes` regex
    from `convertToObject`. Port it verbatim through `regress`.
- **Formatting** follows `JSONGenerator.convertToJSON`:
  `YYYY-MM-DDTHH:mm:ss.SSS` followed by `Z` when the offset is 0, or `±HH:mm`
  otherwise.

### 3.4 `ID_REGEX`, exactly

```js
/^(\p{Lu}|\p{Ll}|\p{Lt}|\p{Lm}|\p{Lo}|\p{Nl}|\$|_|\\u[0-9A-Fa-f]{4})(?:\p{Lu}|\p{Ll}|\p{Lt}|\p{Lm}|\p{Lo}|\p{Nl}|\$|_|\\u[0-9A-Fa-f]{4}|\p{Mn}|\p{Mc}|\p{Nd}|\p{Pc}|‌|‍)*$/u
```

- Port this pattern character for character, compiled once with `regress`
  and the `u` flag. Do not approximate it with `char::is_alphabetic` or
  `is_alphanumeric`. The current `model_util::is_valid_identifier` does that
  and is wrong: it accepts digits outside `Nd` and rejects `Mn`, `Mc`, `Pc`,
  ZWJ and ZWNJ.
- `\\u[0-9A-Fa-f]{4}` matches a **literal** backslash-u escape of six
  characters inside the name. It is not an escape sequence.
- `ID_REGEX.test(undefined)` tests the string `"undefined"` and returns true.
  A caller that passes an absent name (for example a missing `ast.name`)
  reproduces this explicitly (`name.unwrap_or("undefined")`) and records it
  as a `ts-bug` divergence.
- If `regress`'s Unicode tables and the Node ICU version classify a character
  differently, record an `engine` divergence with the code point.

### 3.5 `null` vs `undefined`

- TS often treats them the same (`!x`, `x == null`, `??`, `?.`). Some code
  does not. `StringValidator` tests `minLength === null` (so an absent bound
  behaves differently from a null one), and `NumberValidator` uses
  `hasOwnProperty('lower')`.
- **Inputs.** Use `Option<T>` when the TS code collapses the two. Use
  `Option<Option<T>>` (outer `None` = absent, `Some(None)` = `null`, read with
  a `deserialize_with` helper) when it distinguishes them. Falsy checks (`!x`)
  also catch `""`, `0` and `false`, so port those cases too. For example
  `ModelUtil.getNamespace('')` throws `FQN is invalid.`.
- **Outputs.** The oracle distinguishes them (`null` versus
  `{"@@oracle":"undefined"}`). A Rust function whose TS counterpart returns
  `T | undefined` or `T | null` returns `Option<T>`, and the binding documents
  which JS value `None` becomes. For example `ModelUtil.isEnum` returns
  `undefined` when the type is not found (`typeDeclaration?.isEnum()`). If one
  TS member can return both, use a dedicated enum.
- **JS argument coercion** (a number passed where TS expects a string,
  `undefined` passed to `test`) is done in `concerto-wasm`, per binding, the
  way the TS code applies it. Core takes Rust types. Reproduce a coercion only
  when an oracle fixture or a unit test exercises it.

### 3.6 D6: match TS where it differs from Concerto v4

Where TS v5.0.0 behaves differently from the Concerto v4 specification, the
conformance expectations or the current Rust code, **match TS**, because TS is
the oracle. Examples already known:

- TS `ModelUtil.parseNamespace('org.acme')` accepts an unversioned namespace
  (`version: null`), but the current Rust `parse_namespace` rejects it.
- The two conformance scenarios that expect errors the reference never raises
  (plan §1.2).

Record each difference in `DIVERGENCES.md` (section 7.3) with category `d6`.
The ported test asserts the TS behaviour.

### 3.7 Order

- A JS object's key order is its insertion order for namespace-style keys
  (namespaces contain `@`, so they are never array indices).
  `for (ns in this.modelFiles)` visits namespaces in the order they were
  added. Replacing a value keeps its position, and `delete` followed by a
  re-add moves it to the end. Use an insertion-ordered map (`indexmap`, OD-8)
  and mirror `delete` with `shift_remove`. **Never iterate a `HashMap` on a
  path that can produce an error or a result the oracle observes.** The
  current `ModelManager.model_files` and `ModelFile.local_types` maps must
  change when they are ported.
- Arrays keep AST order. JS `Set` and `Map` keep insertion order. Sort only
  where TS sorts, and with TS's comparator (3.1).

---

## 4. Module layout (`concerto-core`) and the WASM boundary

The target module of each member is the ledger's `target_rust_module`:

```
concerto-core/src/
  lib.rs
  error/            ErrorKind, the error type, the message catalogue and its renderers (P1-05)
  model_util.rs     modelutil.ts
  rootmodel.rs      rootmodelhelper.ts, decoratormodelhelper.ts
  model_manager.rs  basemodelmanager.ts, modelmanager.ts, introspector.ts; the arena, DeclId/PropId, ResolutionContext (P1-04)
  introspect/
    declaration.rs  declaration.ts, classdeclaration.ts and its subclasses, enumdeclaration.ts
    property.rs     property.ts, field.ts, relationshipdeclaration.ts, enumvaluedeclaration.ts
    scalar.rs       scalardeclaration.ts
    map.rs          mapdeclaration.ts, mapkeytype.ts, mapvaluetype.ts
    validators.rs   validator.ts, numbervalidator.ts, stringvalidator.ts, collectionsizevalidator.ts
    decorator.rs    decorated.ts, decorator.ts
    model_file.rs   modelfile.ts
    import.rs       (existing) import handling for modelfile.ts
  instance/         serializer.ts, jsonpopulator.ts, jsongenerator.ts, resourcevalidator.ts,
                    instancegenerator.ts (findConcreteSubclass only), Factory model checks (#32 point 4)
    resource_id.rs  model/resourceid.ts
    metamodel.rs    introspect/metamodel.ts, validateAst
  dcs/              decoratormanager.ts, decoratorextractor.ts (P2-12)
  validation.rs     (existing, crate-private) shared semantic checks
  ecma.rs           crate-private helpers for ECMAScript semantics (3.1): Number::toString,
                    JSON.stringify, UTF-16 length and compare, ToString for params
```

- A task creates the module it needs. It does not create empty modules ahead
  of time (AGENTS.md: no speculative code).
- **WASM-facing handle and JS types stay in `concerto-wasm`, never in core's
  public API.** This keeps the standalone Rust interface (Phase 6, D11) from
  being shaped by the shim.
  - `concerto-core` must not depend on `wasm-bindgen`, `js-sys`,
    `serde-wasm-bindgen` or `web-sys`, and must not use `JsValue`,
    `#[wasm_bindgen]` or wasm-only `cfg` in its public items.
  - The handle registry, the JS-facing error object, the JS-callback
    `ResolutionContext`, the JS implementation of the regex evaluator for
    `options.regExp` (3.2), and argument coercion (3.5) all live in
    `concerto-wasm`.
  - `DeclId` and `PropId` are ordinary core types, because the arena uses them
    natively. Their JS wrappers belong in `concerto-wasm`.
  - Check: `cargo tree -p accordproject-concerto-core -e normal | grep -E 'wasm-bindgen|js-sys|web-sys'`
    prints nothing, and `grep -rn 'wasm_bindgen\|JsValue' concerto-core/src`
    prints nothing.
- Core's public API is idiomatic Rust (`&str`, `Option`, `Result<_, ConcertoError>`,
  iterators). TS-shaped conveniences that exist only for a view belong in
  `concerto-wasm`.

---

## 5. Ledger decisions settled on #32

The maintainer settled the ledger's open questions in three comments on #32:
the approved defaults (1st comment), then the answers to points 1-9 (2nd
comment) and the remaining answers to points 6, 7a and 9 (3rd comment). The
later comments supersede the earlier ones wherever they differ, and #32 says
this file must reflect them. The table gives the final position.

The ledger at `accordproject/concerto` commit `c48423c` already applies
points 1, 2 and 9: its D1 figures use the new denominator, and its
`w_tests`/`direct_tests`/`needs_fallback` columns are the re-derived
coupling. Points 3 to 8 are not yet in `classification.js`, and some TSV
reasons contradict them. For example, the DCS rows still name only P4-09,
`Factory.newResource` is still TS, `DecoratorExtractor.quoteStringValue` is
still HYBRID, and the `Serializer.toJSON`/`fromJSON` reasons still describe
a Serializer-level visitor path kept "for options/tests", which option B
forbids. So **wherever a row's classification or reason differs from a
decision below, this table wins over the TSV**, until `classification.js`
applies points 3 to 8 and the ledger is re-run (OD-6). A HYBRID reason is a
contract (1.3) only when it agrees with this table.

| # | Decision (final) | What it means for a port |
|---|---|---|
| 1 | D1 gate: HYBRID counts at full weight (confirmed). | nothing for implementers. Status reports quote the ledger's published figures (SUMMARY §1, commit `c48423c`): **RUST+HYBRID 85.3%** at full weight against the ≥70% target (met), and **RUST only 57.2%**, over the new denominator of **6498.5** weight. The old 84.1% (all 508 members, denominator 6588.5) and the 1st comment's 70.2% half-weight figure are superseded. Reports must still show the half-weight figure, as the 1st comment required; only its 70.2% value is superseded. The ledger no longer publishes it, so reports quote (3715 + 1826/2) / 6498.5 ≈ **71.2%**, labelled as derived. |
| 2 | **Constant markers and `accept()` are excluded from the D1 denominator**, because they are not logic. *(Changed from the 1st comment, which kept them in.)* The maintainer's note gives −94.5 weight; the rebuilt ledger excludes **60 members, weight 90** (the 53 constant-return rows and the 7 `accept()` rows, SUMMARY §1 and §8 item 2). The 4.5 difference does not move the headline (85.3% either way); quote the ledger's 60 / 90. | still do not port them (1.1 rules 3 and 5). They stay on the TS class unchanged, and are counted neither as TS nor as ported. |
| 3 | DCS in Rust is its own task, **P2-12** (#82), before its view conversion in P4-09 (confirmed) | `concerto_core::dcs` is ported in P2-12, not in P4-09 |
| 4 | **The model checks in `Factory.newResource` are delegated to Rust** (abstract type, identifier type, empty identifier, identifier regex). The Factory becomes HYBRID. Instance creation stays in TS (D7). (confirmed) | the four checks, with their `factory-newinstance-*` messages, become one Rust function in `instance`. Factory (TS) calls it before it builds the object, in the same order as today. The `uuid` and dayjs work and the construction of the `Resource` stay in TS. |
| 5 | **YAML plain-scalar quoting is ported to Rust** with golden tests, so there is no WASM call per string (confirmed) | `DecoratorExtractor.quoteStringValue` becomes fully Rust. Golden tests compare against `yaml.stringify` output, byte for byte. |
| 6 | **Serializer: option B, final.** `Serializer.toJSON`/`fromJSON` always make a single whole-document call into Rust. The internal visitor classes (`JSONGenerator`, `JSONPopulator`, `ResourceValidator`) stay as thin shells whose per-type methods call the same Rust per-field checks. There is one implementation of the rules, in Rust. **Leaving the visitors as untouched legacy TS is not allowed.** | write each per-field check and coercion *once*, in `instance`. The document-level path and the per-field bindings used by the visitor shells call the same function. Never duplicate a check between the two paths, and never leave a visitor method running its own TS check. The visitor shells keep only the dispatch the W tests spy on (`visitX`), so the white-box tests on `jsonpopulator` and `resourcevalidator` exercise the Rust rules. |
| 7 | **Regex: all evaluation against a validator's regex happens in Rust** (`regress`). The facade only converts the pattern into a JS `RegExp` for TS callers of `StringValidator.getRegex()`, and that `RegExp` is never used for validation. *(Changed from the 1st comment.)* | section 3.2 |
| 7a | **The `options.regExp` custom engine: option (b).** Rust evaluates by default. Only when a caller supplies a custom engine through `options.regExp` does evaluation fall back to that JS engine. `getRegex()` converts the pattern into a JS object for TS callers. | section 3.2: the evaluator trait in core, its JS implementation in `concerto-wasm`, and the TS scope of the custom engine (`ScalarDeclaration` validators always use the built-in engine) |
| 8 | `ModelLoader`, `writeModelsToFileSystem` and `updateExternalModels` stay in TS. There is no Rust loader, and the WASM/browser build needs none. (confirmed) | do not port `modelloader.ts`, `writeModelsToFileSystem` or the download in `updateExternalModels`. `updateExternalModels` itself stays HYBRID as in the TSV: the download is TS, and the add, validate and rollback steps run in Rust. There is no Rust loader. |
| 9 | **Coupling: yes, and now.** Re-derive the ledger's coupling from P0-02's per-test tags and the runtime sinon trace, ahead of P2-10 and P4, because it decides where views need a collaborator fallback and which white-box tests need rewriting as fixtures. **Done** in the ledger at commit `c48423c`. | the TSV's `coupled_tests` column is gone. It is replaced by `w_tests`, `direct_tests` and `needs_fallback`, and the old grep survives only as `coupled_tests_grep`, for comparison (1.3). The fallback rows are exactly the `needs_fallback=true` rows: 19 members across 19 classes (SUMMARY §9, section 1.4). The W tests to lift into fixtures are SUMMARY §10: 272 W tests across 24 files, which drives P2-10. SUMMARY §11 lists the 19 of them that map to no ledger member (6.1). A port does not re-derive coupling itself. |

---

## 6. Testing

A unit is done when all of these pass. The exit condition of each P2 or P3
task is the first three items for the units it owns.

### 6.1 Port the TS test cases

- For each TS test file of the unit (for example `test/modelutil.js`), write
  `concerto-core/tests/ported_<stem>.rs` (`tests/ported_modelutil.rs`). Use
  one `#[test]` per TS `it()`, named after its title in snake_case, with the
  exact `describe > it` title as a doc comment. Use inline
  `#[cfg(test)] mod tests` only for crate-private functions.
- Port the **B** and **M** tests (tags in `migration/tags/test-tags.tsv`)
  assertion for assertion: same inputs, same expected values, same expected
  message and kind.
- **W** tests. The ledger's SUMMARY (commit `c48423c`) decides which W tests
  a unit must deal with, and how:
  - **SUMMARY §10** lists every W test to lift, by test file (272 W tests
    across 24 files). These are P2-10's work: P2-10 replaces their sinon
    stubs and spies with real fixtures under `migration/oracle/lifted/`. A P2
    or P3 unit ports a §10 test directly when the behaviour it asserts can
    be reached with real data; otherwise it lists it at the top of the file
    as `// not ported (W): <title>: lifted in P2-10 (SUMMARY §10)`, and
    replaces that with the lifted fixture id once P2-10 has produced it.
  - **SUMMARY §11** lists the 19 W tests that map to no ledger member (bare
    `.ast` reads, and one test with no resolvable stub target). They are
    still in §10's 272, so they are still lifted, but no member's `w_tests`
    column points at them: the unit that owns the test *file* claims them.
    List each as `// not ported (W): <title>: unmapped (SUMMARY §11), lifted
    in P2-10`, or port it if real data reaches the behaviour.
  - **SUMMARY §9** (`needs_fallback`) says which W tests must keep passing
    unchanged in rust mode, rather than waiting for the lift. For the
    flagged **RUST and HYBRID** rows (10 of the 19), the view passes them
    through the collaborator fallback (1.4). For the flagged **TS** rows (9
    of the 19, e.g. `Factory`, `ModelManager`, `JSONPopulator`), there is no
    fallback: the tests pass because that member stays in JS. Either way
    they are never "not ported" at the P4 view task, and
    `CONCERTO_ENGINE=rust` must pass them.
  - A W test that appears in none of §9, §10 or §11 does not exist: every
    W-tagged `it()` in `test-tags.tsv` is in §10 (§11 says so). If a unit
    finds one, the tags or the ledger are stale; raise it on the issue.
  - Use the member's `w_tests` and `direct_tests` columns to find its tests.
    Never use `coupled_tests_grep`.
- **Never edit `packages/concerto-core/test/**`.** A hook enforces this, and
  so does review.
- Keep the existing Rust tests green. Where an existing test asserts
  behaviour that differs from TS (D6), change it to assert the TS behaviour
  and cite the `DV-` id.

### 6.2 Oracle fixtures a unit must pass

A unit is accountable for two sets of fixtures.

- **Own-op fixtures: every fixture whose `op` is `<Class>.<member>` for a
  member the unit ports**, from all sources (`unit`, `data`, `conformance`,
  and later `migration/oracle/lifted/`). List them with
  `ls migration/oracle/fixtures/*/<Class>.<member>/`, and see
  `fixtures/manifest.json` for the counts. Ops of subclasses count too
  (`ClassDeclaration.validate` for `classdeclaration.ts`,
  `AssetDeclaration.declarationKind` and so on). All of them pass at the
  unit's task exit. The `lifted/` fixtures are the ones P2-10 produces from
  the W tests in SUMMARY §10 (including the §11 unmapped ones); once P2-10
  has landed, a lifted fixture for the unit's op is an own-op fixture like
  any other, and it replaces the `not ported (W)` note of 6.1. Before P2-10,
  the note cites SUMMARY §10 or §11 instead.
- **Cross-op fixtures: fixtures of another op whose outcome the unit's code
  decides.** Most validation behaviour is observed under the model-loading
  ops, not under the unit's own ops. For `IllegalModelException`, for
  example, 211 fixtures are under `ModelManager.addCTOModel`, 133 under
  `ModelManager.validateModelFiles` and 225 under
  `DecoratorManager.decorateModels`, against 8 under `MapDeclaration.validate`
  and 11 under `ModelFile.validate`. The model-loading ops are
  `ModelManager.{addModel, addCTOModel, addModelFile, addModelFiles,
  updateModelFile, fromAst, validateModelFile, validateModelFiles}`,
  `ModelFile.new`, `MetaModel.modelManagerFromMetaModel`,
  `DecoratorManager.decorateModels`, and the `Serializer`/`Factory` ops for
  the instance units.
  - **Attribution.** An *error* fixture of a cross op belongs to the unit
    that owns the throw site of its expected message. The P1-07 harness
    attributes it by matching the fixture's `class` and `message` against the
    catalogue (the kind in 2.3, and the rendered template with the class
    decoration of 2.2), and then the entry's recorded `src/<file>.ts:<line>`
    source (OD-10). A fixture that matches no catalogue entry, or more than
    one, is listed as unattributed, and it is never dropped.
  - **When it is due.** An attributed cross-op fixture is due at the unit's
    task exit if the harness can already replay its op natively at that time,
    through the op's dispatch entry and the CTO cache (OD-9). Otherwise the
    PR lists it as deferred to the task that adds the op's dispatch entry,
    and it is due there: P2-08 for the `ModelManager` and `ModelFile` ops,
    P2-12 for the `DecoratorManager` ops, P3-01 for `Serializer` and the
    `Factory` checks. P2-09 (gap audit) confirms that no attributed fixture
    is still deferred.
  - **Success fixtures of a cross op** (the model loads, the instance
    validates), and unattributed ones, belong to the task that owns the op.
  - **No regressions.** A unit must not turn any fixture that passed natively
    before its change into a failure, whatever the fixture's op. That is the
    plan's *Regression* rule (§5.1), applied per PR against the P1-07
    baseline.
- **Natively (P1-07).** The harness in concerto-rust replays the fixtures
  through `cargo test`. Each op the unit ports gets a dispatch entry in the
  harness, in the same PR. An op without an entry is a *failure*, never a
  skip. A fixture that cannot be set up is a harness error, never a pass
  (plan §2.6). Run only the unit's ops while iterating. P1-07 fixes the exact
  command and filter (OD-7).
- **CTO inputs.** 13,006 of the 15,037 fixtures either are
  `ModelManager.addCTOModel` ops or rebuild a model manager whose recipe has
  an `addCTOModel` step (counted with `{"@@oracle":"blob"}` references
  resolved; 12,326 fixture files contain the string `addCTOModel` directly). CTO parsing stays in JS
  (`concerto-cto`), so a native harness cannot rebuild those inputs by
  itself. The harness reads their ASTs from a CTO→AST cache generated in JS
  with the frozen `concerto-cto` 5.0.0 (OD-9). A CTO text missing from the
  cache is a harness error, never a skip or a pass. A recorded parse failure
  replays as the recorded `ParseException`, which Rust never produces (2.3).
- **Through WASM (P4-01, P4-02).** Run
  `node migration/oracle/bin/replay.js --engine <concerto-wasm oracle adapter> --op <Class>.<member>`
  from the concerto checkout. The adapter follows `migration/oracle/README.md`,
  "Adding an engine adapter". The run must pass 100% for the unit's ops.
- **The unit's TS test files in rust mode** (P4 view tasks):
  `CONCERTO_ENGINE=rust migration/bin/run-core-tests.sh --nyc-temp-dir … --report-dir … test/<file>.js`.
  Never run `npm test` directly (see `migration/README.md`).
- Send test output to log files, with `ERROR`-prefixed summary lines
  (plan §5). Run per-file while iterating. Full suites are for runner and
  gate tasks.

### 6.3 Golden tests for messages

- **Every catalogue entry has exactly one golden test** (P1-05 exit
  condition). The expected string is a literal copied from what TS produces
  for the given params: the en.json text passed through `Globalize`, or the
  inline template with values. Rust never generates the expected value.
- Include at least one golden test that exercises the renderer's edge cases:
  a repeated `{param}`, a value that contains `$&` or `$$`, and a value that
  contains another `{param}`.
- Include a completeness test: every en.json key in the catalogue scope
  (OD-5) has an entry, and every entry cites its source.
- If a port adds a message, it adds the entry and its golden test in the
  same PR.

---

## 7. Porting discipline

### 7.1 A faithful port

- Port **behaviour exactly, including its warts**: message wording, check
  order, which phase throws, `undefined` versus `null`, number formatting.
  Make no "improvements": no stricter checks, no nicer messages, no extra
  validation, no reordered checks.
- The Rust may be *structured* idiomatically (sum types, `?`, iterators). The
  *observable behaviour* is the TS behaviour.
- Remove dead code, and add nothing speculative (AGENTS.md).
- New dependencies are limited to the ones listed in OD-8, unless the
  architect approves another on the task issue.

### 7.2 One TS class per task

- A task ports one unit, meaning the ledger rows of one TS class or file.
  Plan tasks that group several classes (for example P2-02: Number, String and
  CollectionSize validators) port them **one class at a time, one commit per
  class**. Each commit includes that class's tests and fixture entries, all
  green.
- Do not touch another unit's code beyond the calls you need. If you need a
  helper from an unported unit, port only that helper, as its own
  `TS: X.y`-marked item, and say so in the PR.

### 7.3 TS bugs and divergences: port them and record them, never fix them silently

If the TS behaviour looks wrong (a TypeError on a missing field, an absent
name accepted as valid, an Infinity accepted as an Integer by the populator):

1. Port it as it is.
2. Add a Rust test that asserts the TS behaviour.
3. Put `// DV-nnn` at the site.
4. Add a row to `DIVERGENCES.md` at the concerto-rust root. The first task
   that needs the file creates it.

```
| id | category | TS 5.0.0 behaviour | expected elsewhere (v4 spec / conformance / "correct") | evidence (fixture id, test title) | Rust site |
```

Categories:

- `ts-bug`: TS behaviour that looks unintended and is ported faithfully.
- `d6`: TS differs from Concerto v4 and TS is matched (3.6).
- `engine`: a JS-engine difference that cannot be avoided, such as regex
  Unicode tables or V8 message text. An `engine` row must not change any
  oracle outcome. If it does, it is a failure, not a divergence. The reviewer
  signs off every `engine` row.

Fixing a TS bug is out of scope for the migration. Open a follow-up issue and
link it from the row.

---

## 8. Worked example: `ModelUtil.getShortName`

**Ledger row:** `src/modelutil.ts ModelUtil getShortName static 9 9 RUST … concerto_core::model_util P2-01+P4-03 logic`.
Rust work belongs to P2-01, and the view to P4-03.

**TS reference** (`src/modelutil.ts`):

```ts
static getShortName(fqn) {
    let result = fqn;
    let dotIndex = fqn.lastIndexOf('.');
    if (dotIndex > -1) {
        result = fqn.substr(dotIndex + 1);
    }

    return result;
}
```

**Rust** (`concerto-core/src/model_util.rs`, which replaces the existing
`short_name`; callers are renamed in the same commit):

```rust
/// Returns everything after the last dot, if present, of the source string.
///
/// TS: ModelUtil.getShortName (src/modelutil.ts)
///
/// ```
/// # use concerto_core::model_util::get_short_name;
/// assert_eq!(get_short_name("org.acme.baz@1.0.0.Foo"), "Foo");
/// assert_eq!(get_short_name("Foo"), "Foo");
/// ```
pub fn get_short_name(fqn: &str) -> &str {
    // `lastIndexOf('.')` + `substr(i + 1)` count UTF-16 units. '.' is one
    // UTF-16 unit and one UTF-8 byte, so the byte split gives the same string.
    match fqn.rfind('.') {
        Some(dot) => &fqn[dot + 1..],
        None => fqn,
    }
}
```

TS `getShortName(undefined)` throws a `TypeError`. No fixture or unit test
exercises it, so the port does not model it (3.5).

**Ported TS tests** (`concerto-core/tests/ported_modelutil.rs`, from
`test/modelutil.js`, where both tests are tagged B):

```rust
/// ModelUtil #getShortName should handle a name with a namespace
#[test]
fn get_short_name_should_handle_a_name_with_a_namespace() {
    assert_eq!(get_short_name("org.acme.baz@1.0.0.Foo"), "Foo");
}

/// ModelUtil #getShortName should handle a name without a namespace
#[test]
fn get_short_name_should_handle_a_name_without_a_namespace() {
    assert_eq!(get_short_name("Foo"), "Foo");
}
```

**Binding** (`concerto-wasm`; illustrative, since P4-01 fixes the naming
scheme):

```rust
#[wasm_bindgen(js_name = modelUtilGetShortName)]
pub fn model_util_get_short_name(fqn: &str) -> String {
    concerto_core::model_util::get_short_name(fqn).to_owned()
}
```

**TS view** (`src/modelutil.ts`, P4-03; P4-02 fixes the engine import and how
`CONCERTO_ENGINE` selects between it and the original body, which stays until
P5-02):

```ts
static getShortName(fqn) {
    return engine().modelUtilGetShortName(fqn);
}
```

The view has one line and no branches, and its signature and JSDoc are
unchanged, so the `.d.ts` snapshot does not move.

**Oracle fixtures.** There are four under
`migration/oracle/fixtures/unit/ModelUtil.getShortName/`, for example
`ddb075231a93fa62f9e3f995.json`:

```json
{"id":"ddb075231a93fa62f9e3f995","source":"unit",
 "source_test":"ModelUtil #getShortName should handle a name with a namespace",
 "op":"ModelUtil.getShortName",
 "inputs":{"args":["org.acme.baz@1.0.0.Foo"]},
 "outcome":{"ok":"Foo"},
 "env":{"random":false},"occurrences":1}
```

The other three are `7ff28776868ae11761a17ca6` (`"Foo"` → `"Foo"`),
`7abf13c2226bf283a6f4d185` (`"org.acme@1.0.0.MyAsset1"` → `"MyAsset1"`,
recorded 15 times from `JSONPopulator` tests) and `6228286ac4a06755bf274c3e`
(`"MyAsset1"` → `"MyAsset1"`). A static op has no `inputs.target`. The native
harness dispatch entry is, in effect,
`"ModelUtil.getShortName" => ok(get_short_name(str_arg(&inputs, 0)?))`.

For comparison, the error form of a sibling op
(`ModelUtil.getNamespace/c577c7d5e493af08097d0f63.json`, called with no
arguments) is
`{"error":{"class":"Error","component":null,"location":null,"message":"FQN is invalid."}}`.
In Rust that is `kind = Error`, `code = "modelutil-getnamespace-nofnq"`, no
params and no location, from `get_namespace(fqn: Option<&str>)` where `None`
or `""` fails the TS `!fqn` check.

**Done for this member when:**

- the ported tests pass;
- the 4 fixtures pass natively and through WASM;
- `test/modelutil.js` passes with `CONCERTO_ENGINE=rust`;
- the rest of the suite stays green with `CONCERTO_ENGINE=ts`.

---

## 9. Open decisions

Neither the plan nor the ledger settles these. Each has a recommended default,
which applies until the maintainer or the architect decides otherwise on the
named task.

| # | Question | Recommended default | Decided in |
|---|---|---|---|
| OD-1 | What is `code`: the message key, or the concerto-util `errorType`? | `code` is the catalogue key. The catalogue entry carries `error_type` for `Validator`/`TypeNotFound` kinds (`DefaultValidatorException`, `RegexValidatorException`, `TypeNotFoundException`). | P1-05 |
| OD-2 | Who applies the exception-class decoration (the `IllegalModelException` file and line suffix, the `Validator error for field …` prefix)? | The shim passes the raw message to the real TS constructor, which decorates it. Rust keeps a verbatim port of each decoration that is used only to compute the final message for the native harness, and golden-tests it against fixtures. | P1-05, P1-07 |
| OD-3 | The generated metamodel types collapse `null` into absent and narrow Integer/Long AST fields to `i32`/`i64`, while TS keeps the JS object (key order, `null`, any number) and exposes it as `.ast` / `getAst()` | Each `ModelFile` keeps the AST it was given as `serde_json::Value` (with `preserve_order`) as the source of truth for `ast()`/`getAst()` and for tri-state reads. The typed `mm::*` view is used for logic. A numeric field that fails to deserialise where TS accepts the model is a failure to fix in `concerto-metamodel` codegen, not in core. | P1-02 |
| OD-4 | The message for an invalid regex comes from the engine (V8 in TS, `regress` in Rust). No fixture or unit test observes it today. | Rust reports `kind = Validator`, `errorType = RegexValidatorException`, with V8's wording (`Invalid regular expression: /<source>/<flags>: <reason>`) for the reasons that regress can map. Record any other reason as an `engine` divergence. | P2-02 |
| OD-5 | Which en.json keys belong in the Rust catalogue? `composer-*`, `whereastvalidator-*`, `like` and `test-*` have no throw site in concerto-core. | Port every key used by a RUST or HYBRID member, plus `factory-newinstance-*` (#32 point 4) and `typenotfounderror-defaultmessage`. Do not port unused keys. `Globalize` stays TS and keeps en.json for them. | P1-05 |
| OD-6 | The ledger at `accordproject/concerto` commit `c48423c` applies #32 points 1, 2 and 9 (new D1 denominator; `w_tests`/`direct_tests`/`needs_fallback` in place of `coupled_tests`), but `classification.js` does not yet apply points 3 to 8: Factory model checks are still TS, `quoteStringValue` is HYBRID, DCS rows point only at P4-09, and the `Serializer.toJSON`/`fromJSON` reasons still describe a Serializer-level visitor path that option B forbids. Also, the P2-12 brief lists "the DCS/YAML converter", but the ledger keeps `dcsconverter.ts` TS (`yaml` npm lib). | Wherever a row's classification or reason differs from any decision in section 5 (points 3 to 8), section 5 overrides the TSV until `classification.js` applies them and the ledger is re-run. For everything else, including the fallback rows (`needs_fallback=true`, 1.4) and the D1 figures (85.3% full weight, 57.2% RUST only, denominator 6498.5), the TSV and SUMMARY at `c48423c` are authoritative as published. P2-12 does the DCS rows. The Factory helper is planned as P3-01 (Rust) and P4-10 (view). `dcsconverter.ts` stays TS unless the maintainer extends #32 point 5 to cover it. | P2-12 / maintainer |
| OD-7 | How does the native harness find the corpus, and how does a task run one op? | Set an env var `CONCERTO_ORACLE_DIR`, defaulting to `../concerto/migration/oracle`, and a filter env var `ORACLE_OP=<Class>.<member>` (a prefix match), run with `cargo test -p accordproject-concerto-core --test oracle`. | P1-07 |
| OD-8 | New dependencies | `regress` (required by the plan), `indexmap` (3.7) and `ryu-js` (3.1) are pre-approved for `concerto-core`. Anything else needs architect approval on the issue. | this rulebook |
| OD-9 | How does the native harness rebuild the fixtures whose inputs are CTO text (13,006 of 15,037, section 6.2), when CTO parsing stays in JS? | P1-07 adds a JS generator in `migration/oracle/` that runs the frozen `concerto-cto` 5.0.0 parser (the one the oracle recorded with) over every CTO text in the corpus, fixture recipes and blobs included. It writes a CTO→AST cache keyed by the SHA-256 of the exact CTO text and the parser arguments that affect the AST, storing either the AST or the recorded `ParseException`. The cache is committed next to the corpus and regenerated whenever the corpus is re-recorded. A check fails if any CTO text in the corpus has no cache entry. The native harness replays an `addCTOModel` step as `add_model` with the cached AST, and never parses CTO in Rust. | P1-07 |
| OD-10 | How are cross-op error fixtures attributed to a unit (section 6.2)? | The P1-07 harness writes an attribution index, `fixture id → catalogue key → src/<file>.ts:<line> → unit`, by matching each error fixture's class and final message against the catalogue (2.2, 2.3). It lists fixtures with no match or several matches as unattributed. The index is regenerated whenever the catalogue changes, and each P2 or P3 PR quotes its unit's slice of it, split into due and deferred (6.2). | P1-07 |

---

## 10. Review checklist

The reviewer works adversarially, from a fresh context, against the TS
source, and not against the implementer's description. **Reject** the port if
any item fails, and cite the item number.

**Scope**
1. The PR ports exactly the ledger rows of its unit(s), one class per commit.
   No TS-classified member is ported. The HYBRID members leave in JS only what
   the ledger `reason` names, with the section 5 overrides applied.
2. Nothing under `packages/concerto-core/test/**` has changed. No nyc
   thresholds or API snapshot changed. No model names appear in files,
   commits or the PR.

**Faithfulness** (read the TS member beside the Rust)
3. Every branch of the TS member has a Rust counterpart, and no check was
   added, dropped or reordered. Constructor-time errors stay load-time errors.
4. Every error has the TS message byte for byte, through the catalogue with
   its golden test. The `kind` maps to the class TS throws (2.3), and the
   location is the verbatim AST `location` that TS passes, or `None`.
5. The semantics rules hold wherever they apply:
   - numbers are `f64`, and text uses JS formatting (3.1);
   - string lengths are UTF-16 (3.1);
   - every validator regex is evaluated in Rust (`regress`, ported flags),
     or through the evaluator trait when `options.regExp` is set, with the
     TS scope of the custom engine (`ScalarDeclaration` validators always
     built-in); the `getRegex()` object is never used for validation (3.2);
   - dates follow dayjs under UTC (3.3);
   - `ID_REGEX` is character for character (3.4);
   - `null`, `undefined` and falsy cases are kept (3.5);
   - maps are insertion-ordered, with no `HashMap` iteration on observable
     paths (3.7).
6. Every TS quirk found is ported, tested, and has a `DV-` row. Nothing is
   silently fixed. A TS recursion over model data is a loop in Rust, and an
   unbounded one returns `JsRangeError` at the TS recursion point, never a
   native stack overflow and never a new cycle error (2.5).

**Structure**
7. Families are enums, only P1-03 traits are used, types are newtypes over
   `mm::*` with no hand-redeclared metamodel fields (1.1, 1.2), and each item
   has a `TS: <Class>.<member>` doc line.
8. Collaborator calls go through `ResolutionContext`. The JS fallback is wired
   for exactly the unit's RUST and HYBRID rows with `needs_fallback=true` in the TSV (SUMMARY
   §9) and no others (1.4, section 5 row 9). A RUST row with the flag still
   has a branch-free one-line view; its collaborator-call path lives only in
   the binding, for that member only.
9. There are no WASM or JS types in core's public API, and the `cargo tree`
   and `grep` checks in section 4 are clean.
10. Serializer checks exist once and are shared by the single-call path and
    the visitor bindings. No visitor method keeps a TS check of its own
    (#32 point 6, option B).

**Evidence**
11. Every `it()` in the unit's TS test files is ported, or listed as not
    ported (W) with the lifted fixture that covers it, or, before P2-10,
    with its SUMMARY §10 or §11 entry (6.1).
12. Every own-op fixture of the unit passes natively, and so does every
    attributed cross-op fixture that is due. The deferred ones are listed
    with the task they are due in, and no fixture that passed before the
    change fails after it (6.2). For P4 tasks, the own-op fixtures also
    pass through WASM, and the unit's TS test files pass with
    `CONCERTO_ENGINE=rust` while the suite stays green with
    `CONCERTO_ENGINE=ts`. The reviewer re-runs at least the native fixtures.
13. `cargo build` and `cargo test` are clean. Every commit is
    DCO-signed, and the PR title uses Conventional Commits.

If the same finding comes up in two reviews, propose a new rule for this file
(plan §5).
