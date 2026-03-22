# Documenting code

- Refer to [Concerto Specification documentation](https://concerto.accordproject.org/docs/category/specification) when figuring out anything related to `concerto-core`, especially anything related to instrospection.
- Add Rust-Docs using above documentation when possible.

# Adding and modifying code

- Always use the most idiomatic approach suitable for Rust language.
- If needed split the code into different crates. If a crate does not exist, ask the operator for help.
- Add a derive macro for repeated `impl`s of a trait, into `cocnerto-macros` crate.
- Types under `concerto-core` should only refer to the types from `concerto-metamodel` using new-type pattern.
- Prefer sum-type over complicated traits.
- Remove any code that is not being used (don't add extra code, just because, that might be useful in the future.)
- Add unit tests for new functions and methods.

# Testing

- Run `cargo build` and `cargo test` to verify any change.

# PR instructions

- Use [Conventional Commits](https://www.conventionalcommits.org/en/v1.0.0/) style for PR titles.
