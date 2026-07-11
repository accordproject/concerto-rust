//! Keeps the generated sources in sync with the Concerto metamodel.
//!
//! `src/metamodel` holds the Rust types the Concerto CLI generates from the
//! metamodel. The CLI version that produced them is recorded in
//! `codegen.version`; while it matches the version pinned here the build does
//! nothing, so routine builds stay offline. Bumping the pin triggers a
//! regeneration, which needs Node.js and network access.

use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

/// The Concerto CLI version the sources are generated with.
const CLI_VERSION: &str = "4.0.2";

/// Records the CLI version the current `src/metamodel` sources came from.
const RECORD: &str = "codegen.version";

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed={RECORD}");

    let root = env::var("CARGO_MANIFEST_DIR").expect("cargo sets CARGO_MANIFEST_DIR");
    let root = Path::new(&root);
    let recorded = fs::read_to_string(root.join(RECORD)).unwrap_or_default();
    if recorded.trim() == CLI_VERSION {
        return;
    }

    let staging = staging_dir();
    generate(&staging);
    install(&staging, &root.join("src").join("metamodel"));
    fs::write(root.join(RECORD), format!("{CLI_VERSION}\n"))
        .expect("failed to record the cli version");
}

/// A scratch directory for the generator output, under cargo's `OUT_DIR`.
fn staging_dir() -> PathBuf {
    let staging = PathBuf::from(env::var("OUT_DIR").expect("cargo sets OUT_DIR")).join("metamodel");
    if staging.exists() {
        fs::remove_dir_all(&staging).expect("failed to clear the staging directory");
    }
    fs::create_dir_all(&staging).expect("failed to create the staging directory");
    staging
}

/// Runs the Concerto CLI code generator into the staging directory.
fn generate(staging: &Path) {
    run(
        Command::new("npx")
            .args(["-y", &format!("@accordproject/concerto-cli@{CLI_VERSION}")])
            .args(["compile", "--metamodel", "--target", "rust", "--output"])
            .arg(staging),
        "npx concerto-cli compile (regenerating the metamodel sources requires Node.js)",
    );
}

/// Moves the generated sources into the crate, replacing the previous set.
///
/// The generator assumes the files sit at the crate root; they live in the
/// metamodel module instead, so crate paths become parent module paths.
fn install(staging: &Path, target: &Path) {
    for entry in fs::read_dir(target).expect("failed to list the generated sources") {
        let path = entry.expect("failed to read a directory entry").path();
        if path.extension().is_some_and(|extension| extension == "rs") {
            fs::remove_file(&path).expect("failed to remove a stale generated source");
        }
    }
    let mut installed = Vec::new();
    for entry in fs::read_dir(staging).expect("failed to list the staging directory") {
        let path = entry.expect("failed to read a directory entry").path();
        let source = fs::read_to_string(&path).expect("failed to read a generated source");
        let destination = target.join(path.file_name().expect("generated files have names"));
        fs::write(&destination, source.replace("use crate::", "use super::"))
            .expect("failed to write a generated source");
        installed.push(destination);
    }
    run(
        Command::new("rustfmt")
            .args(["--edition", "2024"])
            .args(&installed),
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
