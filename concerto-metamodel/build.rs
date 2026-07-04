//! Keeps the generated sources in sync with the Concerto metamodel.
//!
//! The `codegen` directory pins the Concerto packages and holds the script
//! that runs the Rust code generator over the metamodel. This build script
//! hashes those inputs and regenerates `src/metamodel` only when the hash
//! stops matching the recorded one. Routine builds therefore stay offline
//! and skip the generator entirely; bumping a pinned package (or editing
//! the generator script) triggers a regeneration, which requires Node.js.

use sha2::{Digest, Sha256};
use std::env;
use std::fs;
use std::path::Path;
use std::process::Command;

/// The files that determine the generated sources.
const INPUTS: [&str; 3] = [
    "codegen/package.json",
    "codegen/package-lock.json",
    "codegen/generate.js",
];

/// Records the input hash the current `src/metamodel` sources were built from.
const CHECKSUM: &str = "codegen/inputs.sha256";

fn main() {
    for input in INPUTS {
        println!("cargo:rerun-if-changed={input}");
    }

    let root = env::var("CARGO_MANIFEST_DIR").expect("cargo sets CARGO_MANIFEST_DIR");
    let root = Path::new(&root);
    let current = hash_inputs(root);
    let recorded = fs::read_to_string(root.join(CHECKSUM)).unwrap_or_default();
    if recorded.trim() == current {
        return;
    }

    regenerate(root);
    format_generated(root);
    fs::write(root.join(CHECKSUM), current + "\n")
        .expect("failed to record the codegen input checksum");
}

/// Hashes the codegen inputs, length prefixed so file boundaries stay unambiguous.
fn hash_inputs(root: &Path) -> String {
    let mut hasher = Sha256::new();
    for input in INPUTS {
        let bytes =
            fs::read(root.join(input)).unwrap_or_else(|error| panic!("read {input}: {error}"));
        hasher.update(input.as_bytes());
        hasher.update((bytes.len() as u64).to_le_bytes());
        hasher.update(&bytes);
    }
    format!("{:x}", hasher.finalize())
}

/// Installs the pinned packages and runs the code generator.
fn regenerate(root: &Path) {
    let codegen = root.join("codegen");
    run(
        Command::new("npm")
            .args(["ci", "--no-audit", "--no-fund"])
            .current_dir(&codegen),
        "npm ci (regenerating the metamodel sources requires Node.js)",
    );
    run(
        Command::new("node")
            .arg("generate.js")
            .current_dir(&codegen),
        "node generate.js",
    );
}

/// Formats the regenerated sources with rustfmt.
fn format_generated(root: &Path) {
    let generated = root.join("src").join("metamodel");
    let sources = fs::read_dir(&generated)
        .expect("failed to list the generated sources")
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|path| path.extension().is_some_and(|extension| extension == "rs"));
    run(
        Command::new("rustfmt")
            .args(["--edition", "2024"])
            .args(sources),
        "rustfmt over the generated sources",
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
