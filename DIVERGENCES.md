# Divergences

Differences from the TypeScript reference that the port keeps on purpose. See
PORTING.md section 7.3 for the categories and the rules for adding a row.

| id | category | TS 5.0.0 behaviour | expected elsewhere (v4 spec / conformance / "correct") | evidence (fixture id, test title) | Rust site |
|----|----------|--------------------|---------------------------------------------------------|-----------------------------------|-----------|
| DV-001 | engine | TS has no serde step, so no counterpart for which malformed field a deserialisation error names. | With `preserve_order` (required by OD-3), a node with two or more malformed fields reports the first one in the node's own key order. Before P1-02 it reported them in alphabetical key order. No oracle outcome or conformance result changes (60 PASS on both). Accepted by the plan owner on #87. | The P1-02 review compared the base and new builds over 1,736 runs; the only 7 differences are this case (e.g. a class with a malformed `name` and `isAbstract`). | `concerto-core/Cargo.toml` (`serde_json` `preserve_order`) |
