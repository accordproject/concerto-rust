//! `src/generated/` is what `codegen/generate.js` produced from the
//! checked-in inputs (accordproject/concerto-rust#461).
//!
//! `generate.js` records a fingerprint of every input (itself, the npm pin
//! and lock file, the recorded `codegen.version` and the vendored models)
//! and of every file it generates in
//! `src/generated/fingerprints.tsv`. This test recomputes them, so a hand
//! edit to the generated sources, or a change to an input without a
//! regeneration, fails `cargo test` without needing Node.js. CI also runs
//! `node generate.js --check`, which regenerates from the pinned
//! concerto-codegen and compares the result byte for byte.

use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

/// 64-bit FNV-1a of the bytes, carriage returns dropped, as in
/// `generate.js`'s `fingerprint`.
fn fingerprint(bytes: &[u8]) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for &byte in bytes.iter().filter(|&&b| b != b'\r') {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{hash:016x}")
}

#[test]
fn the_generated_sources_match_their_inputs() {
    let crate_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let generated = crate_dir.join("src/generated");
    let recorded = fs::read_to_string(generated.join("fingerprints.tsv"))
        .expect("src/generated/fingerprints.tsv is missing; run `node generate.js` in codegen/");

    let mut listed = BTreeSet::new();
    let mut stale = Vec::new();
    for line in recorded
        .lines()
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
    {
        let (expected, file) = line
            .split_once('\t')
            .unwrap_or_else(|| panic!("malformed fingerprints.tsv line: {line:?}"));
        listed.insert(file.to_string());
        match fs::read(crate_dir.join(file)) {
            Ok(bytes) if fingerprint(&bytes) == expected => {}
            Ok(_) => stale.push(format!("{file} changed")),
            Err(error) => stale.push(format!("{file}: {error}")),
        }
    }

    // Every generated file and vendored model is fingerprinted.
    let mut present: Vec<String> = Vec::new();
    for (dir, prefix, extension) in [
        (&generated, "src/generated", "rs"),
        (&crate_dir.join("vendor"), "vendor", "json"),
    ] {
        for entry in fs::read_dir(dir).unwrap() {
            let name = entry.unwrap().file_name().into_string().unwrap();
            if Path::new(&name).extension().is_some_and(|e| e == extension) {
                present.push(format!("{prefix}/{name}"));
            }
        }
    }
    // The recorded concerto-codegen version `build.rs` compares with the pin.
    present.push("codegen.version".to_string());
    for file in present.iter().filter(|f| !listed.contains(*f)) {
        stale.push(format!("{file} is not in fingerprints.tsv"));
    }

    assert!(
        stale.is_empty(),
        "src/generated/ is not what codegen/generate.js generates from the checked-in \
         inputs; run `npm ci && node generate.js` in concerto-metamodel/codegen \
         (never edit src/generated/ by hand):\n  {}",
        stale.join("\n  ")
    );
}

#[test]
fn the_fingerprint_is_fnv1a_64() {
    // Reference values of 64-bit FNV-1a.
    assert_eq!(fingerprint(b""), "cbf29ce484222325");
    assert_eq!(fingerprint(b"a"), "af63dc4c8601ec8c");
    assert_eq!(fingerprint(b"foobar"), "85944171f73967e8");
    assert_eq!(fingerprint(b"foo\r\nbar"), fingerprint(b"foo\nbar"));
}
