# Concerto Rust

A Rust implementation of the Accord Project [Concerto](https://concerto.accordproject.org)
modeling language, focused on a single reusable validation core that can be
deployed across multiple platforms (native, WASM, and FFI bindings).

## Workspace layout

This repository is a Cargo workspace:

- [`concerto-metamodel`](./concerto-metamodel/): generated Rust types for the
  Concerto metamodel (produced from the upstream `concerto-metamodel` package).
- [`concerto-core`](./concerto-core/): the hand-written core. Holds the
  in-memory representation of Concerto models, with the validation logic to
  follow. Core types wrap the generated metamodel types using the new-type
  pattern.

## Building

```bash
cargo build --workspace
cargo test --workspace
```

## Contributing

See [`AGENTS.md`](./AGENTS.md) for the coding conventions used in this
repository. Pull request titles follow [Conventional Commits](https://www.conventionalcommits.org/),
and commits require a [DCO sign-off](https://github.com/probot/dco#how-it-works)
(`git commit --signoff`).

## License

Apache-2.0. See [LICENSE](./LICENSE).
