use super::*;

// ---- parseUri (private; tested through its effect on fromURI/toURI,
//      and directly for the one case with no other reachable throw
//      site — an authority with a non-numeric port). ----

#[test]
fn parse_uri_splits_scheme_authority_query_and_fragment() {
    let c = parse_uri("resource://user:pass@host:123/a/b?q=1#frag").unwrap();
    assert_eq!(c.protocol.as_deref(), Some("resource"));
    assert_eq!(c.username.as_deref(), Some("user"));
    assert_eq!(c.password.as_deref(), Some("pass"));
    assert_eq!(c.port.as_deref(), Some("123"));
    assert_eq!(c.query.as_deref(), Some("q=1"));
    assert_eq!(c.fragment.as_deref(), Some("frag"));
    assert_eq!(c.path, "/a/b");
}

#[test]
fn parse_uri_with_no_scheme_or_authority_is_all_path() {
    let c = parse_uri("org.acme.l1@1.0.0.Person").unwrap();
    assert_eq!(c.protocol, None);
    assert_eq!(c.path, "org.acme.l1@1.0.0.Person");
}

#[test]
fn parse_uri_non_numeric_port_is_invalid_port() {
    let err = parse_uri("//NOT-A-URI:SUCH-WRONG/rest").unwrap_err();
    assert!(err.to_string().contains("Invalid port"), "{err}");
}

// ---- ResourceId constructor ----

// Lifted from test/model/relationship.js's "#uri serialization" cases,
// which exercise ResourceId only through Relationship (TS has no
// resourceid test file).

#[test]
fn constructor_missing_namespace() {
    let err = ResourceId::new("", "Person", "123").unwrap_err();
    assert!(err.to_string().contains("Missing namespace"), "{err}");
}

#[test]
fn constructor_missing_type() {
    let err = ResourceId::new("org.acme", "", "123").unwrap_err();
    assert!(err.to_string().contains("Missing type"), "{err}");
}

#[test]
fn constructor_missing_id() {
    let err = ResourceId::new("org.acme", "Person", "").unwrap_err();
    assert!(err.to_string().contains("Missing id"), "{err}");
}

// ---- toURI ----

// relationship.js > #uri serialization > check that relationships can
// be serialized to URI
#[test]
fn to_uri_basic() {
    let id = ResourceId::new("org.acme.l1@1.0.0", "Person", "123").unwrap();
    assert_eq!(id.to_uri(), "resource:org.acme.l1@1.0.0.Person#123");
}

// relationship.js > #uri serialization > check that unicode
// relationships can be serialized to URI
#[test]
fn to_uri_unicode() {
    let id = ResourceId::new("org.acme.l1@1.0.0", "Person", "\u{3A9}").unwrap();
    assert_eq!(id.to_uri(), "resource:org.acme.l1@1.0.0.Person#%CE%A9");
}

// ---- fromURI ----

// relationship.js > #uri serialization > check that relationships can
// be created from a URI
#[test]
fn from_uri_resource_scheme() {
    let id = ResourceId::from_uri("resource:org.acme.l1@1.0.0.Person#123", None, None).unwrap();
    assert_eq!(id.namespace, "org.acme.l1@1.0.0");
    assert_eq!(id.type_name, "Person");
    assert_eq!(id.id, "123");
}

// relationship.js > #uri serialization > check that relationships can
// be created from a unicode URI
#[test]
fn from_uri_resource_scheme_unicode() {
    let id = ResourceId::from_uri("resource:org.acme.l1@1.0.0.Person#%CE%A9", None, None).unwrap();
    assert_eq!(id.namespace, "org.acme.l1@1.0.0");
    assert_eq!(id.type_name, "Person");
    assert_eq!(id.id, "\u{3A9}");
}

// relationship.js > #uri serialization > check that relationships can
// be created from a legacy fully qualified identifier
#[test]
fn from_uri_legacy_fully_qualified_no_scheme() {
    let id = ResourceId::from_uri("org.acme.l1@1.0.0.Person#123", None, None).unwrap();
    assert_eq!(id.namespace, "org.acme.l1@1.0.0");
    assert_eq!(id.type_name, "Person");
    assert_eq!(id.id, "123");
}

// relationship.js > #uri serialization > legacy fully qualified
// identifier including tricky characters
#[test]
fn from_uri_legacy_fully_qualified_tricky_id() {
    let id = ResourceId::from_uri("org.acme.l1@1.0.0.Person#1.2:3#4", None, None).unwrap();
    assert_eq!(id.namespace, "org.acme.l1@1.0.0");
    assert_eq!(id.type_name, "Person");
    assert_eq!(id.id, "1.2:3#4");
}

// relationship.js > #uri serialization > check that relationships can
// be created from a legacy identifier
#[test]
fn from_uri_legacy_id_with_explicit_namespace_and_type() {
    let id = ResourceId::from_uri("123", Some("org.acme.l1@1.0.0"), Some("Person")).unwrap();
    assert_eq!(id.namespace, "org.acme.l1@1.0.0");
    assert_eq!(id.type_name, "Person");
    assert_eq!(id.id, "123");
}

// relationship.js > #uri serialization > should error on invalid URI
// scheme
#[test]
fn from_uri_invalid_scheme() {
    let err = ResourceId::from_uri("banana:org.acme.l1@1.0.0.Person#123", None, None).unwrap_err();
    assert!(err.to_string().contains("banana"), "{err}");
}

// relationship.js > #uri serialization > should error on invalid URI
// content
#[test]
fn from_uri_invalid_uri_content() {
    let uri = "resource://NOT-A-URI:SUCH-WRONG/org.acme.l1@1.0.0.Person#123";
    let err = ResourceId::from_uri(uri, None, None).unwrap_err();
    assert!(
        err.to_string()
            .contains("Invalid URI: resource://NOT-A-URI:SUCH-WRONG"),
        "{err}"
    );
}

// relationship.js > #uri serialization > should error on URI content
// that Composer does not support
#[test]
fn from_uri_unsupported_authority() {
    let uri = "resource://USER:PASSWORD@HOSTNAME:1567/org.acme.l1@1.0.0.Person#123";
    let err = ResourceId::from_uri(uri, None, None).unwrap_err();
    assert!(
        err.to_string()
            .contains("Invalid resource URI format: resource://USER:PASSWORD@HOSTNAME:1567"),
        "{err}"
    );
}

// relationship.js > #uri serialization > should error on missing
// namespace in URI
#[test]
fn from_uri_missing_namespace() {
    let err = ResourceId::from_uri("resource:Person#123", None, None).unwrap_err();
    assert!(err.to_string().contains("Missing namespace"), "{err}");
}

// relationship.js > #uri serialization > should error on missing type
// in URI
#[test]
fn from_uri_missing_type() {
    let err = ResourceId::from_uri("resource:org.acme.l1@1.0.0.#123", None, None).unwrap_err();
    assert!(err.to_string().contains("Missing type"), "{err}");
}

// relationship.js > #uri serialization > should error on missing ID
#[test]
fn from_uri_missing_id() {
    let err = ResourceId::from_uri("", Some("org.acme.l1@1.0.0"), Some("Person")).unwrap_err();
    assert!(err.to_string().contains("Missing id"), "{err}");
}

// Round-trip: toURI then fromURI recovers the same triple, for both
// ASCII and unicode ids.
#[test]
fn round_trip_ascii() {
    let id = ResourceId::new("org.acme.l1@1.0.0", "Person", "123").unwrap();
    let round_tripped = ResourceId::from_uri(&id.to_uri(), None, None).unwrap();
    assert_eq!(id, round_tripped);
}

#[test]
fn round_trip_unicode() {
    let id = ResourceId::new("org.acme.l1@1.0.0", "Person", "\u{3A9}").unwrap();
    let round_tripped = ResourceId::from_uri(&id.to_uri(), None, None).unwrap();
    assert_eq!(id, round_tripped);
}
