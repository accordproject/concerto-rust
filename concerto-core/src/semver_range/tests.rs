use super::*;

#[test]
fn partial_version_pattern_compiles() {
    LazyLock::force(&PARTIAL_VERSION_REGEX);
}

#[test]
fn caret_and_tilde() {
    assert!(satisfies("5.0.0", "^5.0.0", true));
    assert!(satisfies("5.9.9", "^5.0.0", true));
    assert!(!satisfies("6.0.0", "^5.0.0", true));
    assert!(!satisfies("5.0.0", "^0.80", true)); // 5.0.0 is well outside ^0.80.x
    assert!(satisfies("0.80.4", "^0.80", true));
    assert!(!satisfies("0.81.0", "^0.80", true));
    assert!(satisfies("1.2.9", "~1.2.3", true));
    assert!(!satisfies("1.3.0", "~1.2.3", true));
}

#[test]
fn space_separated_and_and_or() {
    assert!(satisfies("5.0.0", ">=3.0.0 <6.0.0", true));
    assert!(!satisfies("6.0.0", ">=3.0.0 <6.0.0", true));
    assert!(satisfies("5.0.0", "^1.0.0 || ^5.0.0", true));
}

#[test]
fn hyphen_ranges() {
    assert!(satisfies("5.0.0", "3.0.0 - 6.0.0", true));
    assert!(satisfies("5.0.0", "3.0.0 - 5", true));
    assert!(!satisfies("6.0.0", "3.0.0 - 5", true));
}

#[test]
fn bare_version_is_exact_not_caret() {
    assert!(satisfies("5.1.0", "5.1.0", true));
    assert!(!satisfies("5.1.1", "5.1.0", true));
}

#[test]
fn x_ranges() {
    assert!(satisfies("5.4.0", "5.x", true));
    assert!(!satisfies("6.0.0", "5.x", true));
    assert!(satisfies("5.4.9", "5.4", true));
    assert!(satisfies("5.4.0", "*", true));
}

#[test]
fn an_unparseable_range_is_not_satisfied() {
    assert!(!satisfies("5.0.0", "not a range", true));
}
