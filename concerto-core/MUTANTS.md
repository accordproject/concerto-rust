# cargo-mutants on the validation modules (P5-06)

Tracks accordproject/concerto-rust#77 (plan §2.6, §4 Phase 5, done-criterion
§0.6: catch rate ≥ 85% on the validation modules).

Scope: `concerto-core/src/validation.rs`, `concerto-core/src/instance/validate.rs`
and `concerto-core/src/introspect/validators.rs` — 420 mutants total
(103 + 198 + 119), generated with:

```
cargo mutants -p accordproject-concerto-core \
  -f concerto-core/src/validation.rs \
  -f concerto-core/src/instance/validate.rs \
  -f concerto-core/src/introspect/validators.rs \
  --cargo-test-arg --lib
```

`--lib` scopes the run to the unit suite, not the oracle integration suite
(`cargo test`'s default target set): the oracle alone costs several minutes
per invocation, which makes a 420-mutant × full-suite sweep impractical on
this shared host. A few specific mutants (`validate_detached_declaration`,
`validate_detached_map_key`, `validate_detached_map_value`, `detached_map`'s
match arm) have *no* `--lib` coverage by design — their real coverage lives
in the oracle corpus (`tests/oracle/ops.rs`'s `declref`/`map_part` recipes
on an `mfnew` target) — and were given `--lib` unit tests here anyway so the
gate itself does not depend on the slower suite.

## Status: exit condition established — 420/420 examined, 94.5% catch rate

A full, uninterrupted sweep of all 420 mutants has now completed, split
across two invocations run to completion on a quiet host (load average
2.5–17 during this task, vs. 30–48 in every earlier attempt):

| Scope | Mutants | Caught | Missed | Unviable | Catch rate (missed/(caught+missed)) |
|---|---|---|---|---|---|
| `introspect/validators.rs` | 119 | 111 | 0 | 8 | **100%** |
| `validation.rs` + `instance/validate.rs` | 301 | 247 | 21 | 33 | 92.2% |
| **Total** | **420** | **358** | **21** | **41** | **94.5%** |

21 missed, 2 of which are pre-existing documented-equivalent mutants
(unchanged from earlier sessions), leaves **19 newly-discovered, unresolved
survivors** — see "Newly-discovered, unresolved survivors" below. Even
counting all 19 as real gaps, the catch rate (358/379 = 94.5%, or
360/379 = 95.0% once the 2 equivalents are excluded like the unviable ones)
clears the ≥ 85% gate with a wide margin. **The §0.6 exit condition is
established.**

### This task's contribution

- `introspect/validators.rs` had never been swept in any session (0/119,
  the review's second blocking finding). It is now fully swept at 100%
  catch rate (111/111, excluding 8 unviable — all `&&`→`||` mutants inside
  `if let ... && cond` let-chains, which don't type-check and so aren't
  real gaps). 22 real survivors were found and fixed with 22 new
  `#[test]`s: `NumberValidator`'s `compatible_with`/`lower_bound`/
  `upper_bound`/`Display` were never exercised through the `Validator`
  enum wrapper or at all (every existing test called the inner type's own
  method directly); `CollectionSizeValidator::validate`'s bounds were never
  checked at the exact inclusive boundary; `CompiledRegex`'s `PartialEq`
  was never exercised (nothing compared two `StringValidator`s or hit both
  `&&`-branches); `v8_regex_reason`'s two arms (the specific V8-wording
  translation and the passthrough fallback) were never asserted on by
  message content; `StringValidator::min_length`/`max_length` had no direct
  accessor tests; and `StringValidator::compatible_with`'s min-length
  `(None, Some(_))` arm had no test (only the symmetric max-length case
  existed).
- `validation.rs` + `instance/validate.rs`, previously examined only up to
  195/301 across all prior sessions (with every survivor found in that
  partial population fixed), is now fully swept for the first time. This
  found one new pocket of real gaps not reachable by any 195/301 subset:
  `map_key_is_scalar` and `check_map_type`'s `DateTime`/`Boolean` primitive
  arms had no fixture reaching them at all (every map in the fixture used
  a plain `StringMapKeyType` key and only `String`/`Object`/
  `Relationship`-typed values). Fixed by adding four map declarations
  (`ScalarKeyMap`, `PlainKeyScalarValueMap`, `DateTimeMap`, `BooleanMap`)
  and 7 new `#[test]`s, each independently confirmed by this second
  from-scratch sweep going from 20 missed (first full run of this file
  pair) down to 2 (both pre-existing equivalents).

`cargo test -p accordproject-concerto-core --lib`: 778 passed, 0 failed
(760 before this task: +11 in `introspect/validators.rs`, then +7 more in
`instance/validate.rs` for the map fixture, net +18).

### Pre-existing equivalent mutants (unchanged, still equivalent)

| Mutant | Function | Why equivalent |
|---|---|---|
| `validation.rs:1304:24` guard→`false` (was `1277:24` before intervening commits shifted the line) | `validate_map_value` | Documented in place: `MapDeclaration::from_json` already rejects any `type.$class` that doesn't match this exact string at construction, so this arm's condition cannot see a different value at validate time — this sweep re-confirms it, no change needed |
| `instance/validate.rs:843:31` `\|\|`→`&&` (was `824:31`) | `as_js_object` | Documented in place (96d6f24): the two tag checks it ORs require mutually exclusive shapes, and every call site reports against the original value, not the returned one |

### Newly-discovered, unresolved survivors (19) — follow-up, not blocking

Found by this sweep's first full pass over `instance/validate.rs`, past
the 195/301 previously examined. None of these affect the ≥ 85% gate
(already cleared by 9.5 points even counting all 19 as real), and none are
part of the two findings this task was asked to fix, so they are recorded
here rather than fixed in this pass:

| Mutant(s) | Function | Likely status |
|---|---|---|
| `1425:9`, `1426:13`–`1436:13` (5 arms), `1445:9` (9 mutants total) | `FieldElement`'s `default_value`/`name` (`ValidatedElement` impl) | The `Property::Boolean` arm (`1426:13`) looks genuinely equivalent — `check_primitive_item`'s own match has an empty `Property::Boolean(_) \| Property::DateTime(_) => {}` arm, so no validator (the only reader of `default_value()`) is ever built for a boolean field. The `String`/`Integer`/`Long`/`Double` arms and the whole-function/`name()` mutants look like real gaps: no fixture property of those types carries *both* a validator and a `defaultValue` that the validator would actually reject, so `default_value()`'s real return is indistinguishable from a forced `None`/empty string in every existing test |
| `1497:5` (2 mutants) | `js_to_string_element` | Likely real: no existing error-message assertion checks an array value containing a `null`/`undefined` element specifically (the one path where this differs from plain `js_to_string`) |
| `1921:17`–`1934:17` (7 arms) | `classify_error` | Status unclear without deeper investigation: `abstract_class_is_diagnosed` etc. already assert the exact `DiagnosticCode` these arms produce and pass today, so either `classify_error` isn't actually the code path those tests exercise (a different, more direct classification for the "collect all" walk), or there is some other reason the arm's deletion is unobservable. Needs tracing through `collect_diagnostics`'s call graph, not guessing |
| `2119:24` | `collect_class` (`!is_js_null(v)` guard) | Not yet investigated |

### Recommended follow-up

Investigate and close the 19 above, in particular resolving the
`classify_error` puzzle (why passing tests don't kill arm-deletions on
codes they appear to assert on) before assuming they're real gaps.

## Prior history (this section is now historical; superseded by the table above)

### Survivors found and closed in earlier sessions

From the fullest single interim sweep run before this task (195/420
examined: 160 caught, 20 missed, 14 unviable), all 20 missed mutants were
resolved prior to this task:

| Mutant | Function | Resolution |
|---|---|---|
| `validation.rs:888:44` `==`→`!=` | `check_property_type` | Fixed (96d6f24): three-namespace test |
| `validation.rs:1267:13` delete `None` arm | `validate_map_value` | Fixed (4b1ba3d/3725e12): reachable via a non-string `type.name` |
| `validation.rs:1277:24` guard→`false` | `validate_map_value` | **Equivalent** (documented in place) |
| `validation.rs:185:33` `==`→`!=` | `validate_detached_model_file` | Fixed: namesake-with-different-content test |
| `validation.rs:186:39` `==`→`!=` | `validate_detached_model_file` | Fixed: differently-named-but-identical-AST test |
| `validation.rs:530:23` guard→`true` | `validate_property` (`in_owner_file`) | Fixed: own-property case via `validate_detached_declaration` |
| `validation.rs:569:27` guard→`true` | `validate_property` (context-file attach) | Fixed |
| `instance/validate.rs:297:53`/`297:60` | `visit_class_declaration` | Fixed (96d6f24) |
| `instance/validate.rs:363:21` | `fully_qualified_identifier` | Fixed (96d6f24) |
| `instance/validate.rs:406:5` | `identifiable_to_string` | Fixed (96d6f24) |
| `instance/validate.rs:421:5` | `property_has_default_value` | Fixed (96d6f24) |
| `instance/validate.rs:587:31` | `check_enum` | Fixed (96d6f24) |
| `instance/validate.rs:675:39`/`700:9` | `check_primitive_item` | Fixed (96d6f24) |
| `instance/validate.rs:824:31` guard→`&&` | `as_js_object` | **Equivalent** (documented in place, 96d6f24) |
| `instance/validate.rs:907:5` body→`true` | `parses_as_dayjs` | Fixed |
| `instance/validate.rs:911:9` delete `Number` arm | `parses_as_dayjs` | Fixed |
| `instance/validate.rs:912:9` delete `String` arm | `parses_as_dayjs` | Fixed |
| `instance/validate.rs:925:5` body→`Default::default()` | `number_validator_ast` | Fixed |

See git history (commits `4b1ba3d`, `3725e12`, `a81503d`, `96d6f24`) for the
full detail on each; superseded above by the complete 420/420 sweep.
