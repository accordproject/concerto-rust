# Concerto Rust

Rust implementation of the Concerto modeling language, focusing on structural and semantic metamodel validation.

## Overview

This project is a partial Rust implementation of the [Concerto](https://github.com/accordproject/concerto) modeling language, which was originally developed in JavaScript. This Rust version focuses specifically on the structural and semantic validation aspects of the metamodel.

## Project Structure

The project is organized into different crates under one workspace:

- [`concerto-core`](/concerto-core/) is the core implementation with introspection and validation.
- [`concerto-macros`](/concerto-macros/) has the derive macros used by the `cocnerto-core` crate.
- [`concerto-metamodel`](/concerto-metamodel/) crate exposes the generated Rust types from the main [`concerto-metamodel`](https://github.com/accordproject/concerto-metamodel) package.

## Usage

TBD

## License

This project is licensed under the Apache 2.0 License.
