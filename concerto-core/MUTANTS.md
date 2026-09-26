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
this shared host (see "Known incompleteness" below). A few specific mutants
(`validate_detached_declaration`, `validate_detached_map_key`,
`validate_detached_map_value`, `detached_map`'s match arm) have *no* `--lib`
coverage by design — their real coverage lives in the oracle corpus
(`tests/oracle/ops.rs`'s `declref`/`map_part` recipes on an `mfnew` target) —
and were given `--lib` unit tests here anyway so the gate itself does not
depend on the slower suite.

## Status: partial — exit condition not established by a completed sweep

Every mutant *examined so far*, across all sessions that have worked this
task, is accounted for: fixed with a real test, or documented in place as
genuinely equivalent. But the full 420-mutant population has still never
been examined in one sweep — see "Known incompleteness".

### Survivors found and closed

From the fullest single interim sweep run to date (195/420 examined: 160
caught, 20 missed, 14 unviable), all 20 missed mutants are now resolved:

| Mutant | Function | Resolution |
|---|---|---|
| `validation.rs:888:44` `==`→`!=` | `check_property_type` | Fixed (96d6f24): three-namespace test |
| `validation.rs:1267:13` delete `None` arm | `validate_map_value` | Fixed (4b1ba3d/3725e12): reachable via a non-string `type.name` |
| `validation.rs:1277:24` guard→`false` | `validate_map_value` | **Equivalent** (documented in place): construction already rejects any non-matching `$class`, so this arm cannot see a different value |
| `validation.rs:185:33` `==`→`!=` | `validate_detached_model_file` | Fixed (this task): namesake-with-different-content test |
| `validation.rs:186:39` `==`→`!=` | `validate_detached_model_file` | Fixed (this task): differently-named-but-identical-AST test |
| `validation.rs:530:23` guard→`true` | `validate_property` (`in_owner_file`) | Fixed (this task): own-property case via `validate_detached_declaration`, which never runs the outer `attach` |
| `validation.rs:569:27` guard→`true` | `validate_property` (context-file attach) | Fixed (this task): same shape, the `context_ns` guard. **Corrects 3725e12's "genuinely equivalent" claim about this pair — it was wrong**: the guard's false branch is observable through `validate_detached_declaration`, which the two tests supporting that original claim never exercised |
| `instance/validate.rs:297:53`/`297:60` | `visit_class_declaration` | Fixed (96d6f24) |
| `instance/validate.rs:363:21` | `fully_qualified_identifier` | Fixed (96d6f24) |
| `instance/validate.rs:406:5` | `identifiable_to_string` | Fixed (96d6f24) |
| `instance/validate.rs:421:5` | `property_has_default_value` | Fixed (96d6f24) |
| `instance/validate.rs:587:31` | `check_enum` | Fixed (96d6f24) |
| `instance/validate.rs:675:39`/`700:9` | `check_primitive_item` | Fixed (96d6f24) |
| `instance/validate.rs:824:31` guard→`&&` | `as_js_object` | **Equivalent** (documented in place, 96d6f24): the two tag checks it ORs require mutually exclusive shapes, and every call site reports against the original value, not the returned one |
| `instance/validate.rs:907:5` body→`true` | `parses_as_dayjs` | Fixed (this task) — not mentioned in 96d6f24 despite being in the same missed list |
| `instance/validate.rs:911:9` delete `Number` arm | `parses_as_dayjs` | Fixed (this task) |
| `instance/validate.rs:912:9` delete `String` arm | `parses_as_dayjs` | Fixed (this task) |
| `instance/validate.rs:925:5` body→`Default::default()` | `number_validator_ast` | Fixed (this task): `validator_failure_is_diagnosed` only ever exercised the out-of-range side, where a construction error and a real validation failure report the same diagnostic code |

18 of 20 fixed with a real killing test; 2 documented as genuinely
equivalent. Every fix from this task (185, 186, 530, 569, 907, 911, 912,
925) was independently confirmed by manually applying that exact mutation
to a scratch copy of the source, re-running just the new test, observing
it fail, then reverting — the same rigor as a scoped `cargo-mutants -F`
re-run, without the cost of one.

### `validate_property`'s `owner_ns`/`context_ns` guards: 3725e12 was wrong

3725e12 asserted the `owner_ns != namespace` / `context_ns != namespace`
guards were equivalent to a constant `true` "when the guard is naturally
true in every reachable case (`owner_ns == namespace` implies the looked-up
file IS the file already being validated)". That reasoning holds for every
entry point that runs `validate_model_file_with_import_scope`'s outer
`attach` closure (`validate_model_file`, `validate_detached_model_file`):
the outer closure only sets `model_file` when it is still unset, so an
early inner attach of the *same* file is unobservable there.

It does not hold for `ModelManager::validate_detached_declaration`, which
validates one declaration directly and never runs that outer closure. On
that path a wrongly-forced-`true` guard is the *only* thing that attaches a
file to the error at all — and it attaches the wrong one, changing
`final_message()`'s "File '…': " prefix. The two existing tests that this
guard pair already had
(`a_duplicate_decorator_on_an_inherited_property_is_attached_to_the_owner_file`,
`an_inherited_relationship_to_an_unidentified_type_is_attached_to_the_types_file`)
only ever exercise the guards' *true* branch (an inherited, other-namespace
property), never the *false* branch the mutant actually changes, which is
why cargo-mutants kept finding them uncaught through every subsequent
sweep despite the equivalence claim.

## Known incompleteness

A full, uninterrupted 420-mutant sweep has not been completed end to end in
any session to date, this one included. All three attempts (4b1ba3d/
3725e12, 96d6f24, and this one) were slowed or stopped by the same cause:
this is a shared host, and concurrent sessions' own `cargo build`/`cargo
test`/`cargo mutants` runs push the load average well past the core count
(30–48 on 8 cores was observed during this task), multiplying every
mutant's build+test cost and, in the worst case, exhausting disk with
concurrent temp build directories.

What is and is not established as a result:

- **Established** (independently confirmed, not just reasoned about): every
  mutant in the table above, plus everything the original 195/420 sweep
  examined and already caught (160 mutants) or found unviable (14).
- **Not established**: the remaining population never generated/examined by
  any sweep — the back half of `validation.rs` and most of
  `instance/validate.rs` beyond the specific lines above, and *all* of
  `introspect/validators.rs` (119 mutants, never swept in any session).
  `introspect/validators.rs` is extensively unit-tested already (33
  `#[test]`s covering `NumberValidator`, `StringValidator` and
  `CollectionSizeValidator` construction, `compatibleWith` and regex/flag
  handling), so a real gap there is plausible but unconfirmed either way.

The ≥ 85% catch-rate gate in §0.6 is therefore **not yet established** by a
completed measurement. Given the fix rate on every mutant actually examined
so far (18/20 fixed, 2 proven equivalent — effectively 100% once equivalent
mutants are excluded from the denominator, the usual cargo-mutants
convention), there is no evidence of a systemic gap; the risk is entirely in
the ~225 mutants no session has generated or run yet.

### Recommended follow-up

- Resume the sweep on a quiet host (or with `-j` capped low enough not to
  contend with concurrent sessions) using the same command above; consider
  sharding (`--shard i/N`) across several quieter windows rather than one
  long foreground run.
- Prioritise `introspect/validators.rs` first, since it has zero sweep
  history in any session.
- `baseline.tsv`/the oracle corpus are unaffected by any of this — this
  gate is orthogonal to the oracle pass/fail gate.
