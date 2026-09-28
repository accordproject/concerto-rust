# Concerto Rust

A Rust implementation of the Accord Project [Concerto](https://concerto.accordproject.org)
modeling language, focused on a single reusable validation core that can be
deployed across multiple platforms (native, WASM, and FFI bindings).

## Workspace layout

This repository is a Cargo workspace:

- [`concerto-core`](./concerto-core/): Runtime for Concerto. Holds the
  in-memory representation of Concerto models, implements type validation.
- [`concerto-core-js`](./concerto-core-js/): the JS object model (the TS
  `Resource` objects and the JS values they hold, `Serializer`, `Factory`)
  that the WASM binding (`concerto-wasm`) is built on. Not published; a
  native caller does not need it (`docs/public-api.md` section 4.6).
- [`concerto-vocabulary`](./concerto-vocabulary/): Runtime for Concerto Vocabularies.
  **To be implemented.**
- [`concerto-metamodel`](./concerto-metamodel/): generated Rust types for the
  Concerto metamodel (produced from the upstream `concerto-metamodel` package).

## Building

```bash
cargo build --workspace
cargo test --workspace
```

## Contributing

Pull request titles follow [Conventional Commits](https://www.conventionalcommits.org/),
and commits require a [DCO sign-off](https://github.com/probot/dco#how-it-works)
(`git commit --signoff`).

See [`AGENTS.md`](./AGENTS.md) for the coding conventions used in this
repository.

## License

Apache-2.0. See [LICENSE](./LICENSE).
