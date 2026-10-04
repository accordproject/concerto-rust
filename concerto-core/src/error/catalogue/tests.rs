use super::*;

/// Every entry's `code` is unique, it cites its source, and (`"pre-port"`
/// excepted) it has a golden test in `error/tests.rs`, named after its code
/// (checked by name, PORTING.md 6.3).
///
/// Templates need not be unique: `en.json` gives two keys
/// (`modelfile-constructor-unrecmodelelem`,
/// `classdeclaration-process-unrecmodelelem`) the same text, and a
/// fixture is attributed by `code`.
#[test]
fn catalogue_is_complete() {
    let golden_tests_source = include_str!("../tests.rs");
    for (i, entry) in CATALOGUE.iter().enumerate() {
        assert!(!entry.sources.is_empty(), "{} cites no source", entry.code);
        assert!(
            CATALOGUE[..i].iter().all(|e| e.code != entry.code),
            "{} is duplicated",
            entry.code
        );
        if entry.code == "pre-port" {
            continue;
        }
        let golden = format!("fn golden_{}()", entry.code.replace('-', "_"));
        assert!(
            golden_tests_source.contains(&golden),
            "{} has no golden test",
            entry.code
        );
    }
}

/// The `"pre-port"` entry itself is tested (`golden_pre_port`, error/tests.rs),
/// but is exempt from the by-name check above because its code does not
/// spell a TS message key.
#[test]
fn pre_port_entry_uses_the_raw_renderer() {
    let entry = catalogue_entry("pre-port").expect("pre-port entry must exist");
    assert_eq!(entry.renderer, Renderer::Raw);
}
