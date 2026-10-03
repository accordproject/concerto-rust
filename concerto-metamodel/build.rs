//! Keeps the generated sources in sync with the pinned concerto-codegen.
//!
//! `src/generated` holds the Rust types concerto-codegen's Rust target
//! generates from the models in `vendor/` (through `codegen/generate.js`).
//! The concerto-codegen version that produced them is recorded in
//! `codegen.version`; while it matches the version pinned in
//! `codegen/package.json` the build does nothing, so routine builds stay
//! offline and need no Node.js. Bumping the pin triggers a regeneration,
//! which needs Node.js and network access. Drift between the committed
//! sources and their inputs is caught by `tests/drift.rs` and by
//! `node codegen/generate.js --check`, not here.

use std::env;
use std::fs;
use std::path::Path;
use std::process::Command;

/// The npm package the sources are generated with.
const CODEGEN: &str = "@accordproject/concerto-codegen";

/// Records the concerto-codegen version the current sources came from.
const RECORD: &str = "codegen.version";

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed={RECORD}");
    println!("cargo:rerun-if-changed=codegen/package.json");

    let root = env::var("CARGO_MANIFEST_DIR").expect("cargo sets CARGO_MANIFEST_DIR");
    let root = Path::new(&root);
    let manifest = fs::read_to_string(root.join("codegen/package.json"))
        .expect("failed to read codegen/package.json");
    let manifest: serde_json::Value =
        serde_json::from_str(&manifest).expect("codegen/package.json is not JSON");
    let pinned = manifest["dependencies"][CODEGEN]
        .as_str()
        .unwrap_or_else(|| panic!("codegen/package.json does not pin {CODEGEN}"));
    let recorded = fs::read_to_string(root.join(RECORD)).unwrap_or_default();
    if recorded.trim() == pinned {
        return;
    }

    let codegen = root.join("codegen");
    run(
        Command::new("npm").arg("ci").current_dir(&codegen),
        "npm ci (regenerating the metamodel sources requires Node.js)",
    );
    run(
        Command::new("node")
            .arg("generate.js")
            .current_dir(&codegen),
        "node codegen/generate.js",
    );
}

fn run(command: &mut Command, what: &str) {
    let status = command
        .status()
        .unwrap_or_else(|error| panic!("failed to run {what}: {error}"));
    if !status.success() {
        panic!("{what} exited with {status}");
    }
}
