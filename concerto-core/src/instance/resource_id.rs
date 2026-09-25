//! A resource's identity as a URI: a port of `ResourceId` and its private
//! `parseUri` helper (`src/model/resourceid.ts`) from the TypeScript
//! reference.
//!
//! `ResourceId` names a resource with three parts — `namespace`, `type` and
//! `id` — and knows how to read and write them as a `resource:` URI
//! (`resource:qualifiedTypeName#ID`), including the legacy formats that
//! carry the id alone. `parseUri` is the minimal RFC 3986 parser
//! `ResourceId::from_uri` builds on: scheme, authority (userinfo/host/port),
//! query and fragment, leaving the remainder as the path. It stays private
//! (`crate`-visible only), matching `parseUri`'s own `@private` marker in TS.
//!
//! Ledger: `src/model/resourceid.ts` (`parseUri`, `ResourceId` constructor,
//! `fromURI`, `toURI`), `planned_task` P2-01+P4-03 (`SEAM_LEDGER.tsv`).
//! `ResourceId` is otherwise untested directly in TS: its only coupled
//! tests are `test/model/relationship.js`'s "#uri serialization" cases,
//! reached through `Relationship.fromURI`/`toURI`. This port's tests lift
//! the ones that exercise `ResourceId` itself (as opposed to
//! `ModelManager.getType`, which is `Relationship`'s own job and stays out
//! of scope here).

use crate::error::{ConcertoError, ContractError, ErrorKind, Result};
use crate::model_util;

/// `RESOURCE_SCHEME` in `src/model/resourceid.ts`.
const RESOURCE_SCHEME: &str = "resource";

/// A [`ContractError`] as the crate's error type.
fn error(
    kind: ErrorKind,
    code: &'static str,
    params: Vec<(&'static str, String)>,
) -> ConcertoError {
    ContractError::new(kind, code, params).into()
}

/// The pieces `parseUri` splits a URI into. Every field but `path` is `None`
/// where TS leaves the variable `null`; `path` is always present (TS
/// initialises `path` from the remainder unconditionally, never `null`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct UriComponents {
    protocol: Option<String>,
    username: Option<String>,
    password: Option<String>,
    port: Option<String>,
    query: Option<String>,
    fragment: Option<String>,
    path: String,
}

/// `Some(s)` is JS-truthy only when `s` is non-empty; `None` and `Some("")`
/// are both falsy, matching how TS reads `uriComponents.username` and
/// friends (declared nullable but sometimes assigned `""`, e.g. an empty
/// userinfo before a bare `@`).
fn is_present(value: &Option<String>) -> bool {
    matches!(value, Some(s) if !s.is_empty())
}

/// Parse a URI into its component parts. Implements the subset of the
/// generic URI parsing algorithm (RFC 3986) that `ResourceId` relies on:
/// fragment, query, scheme, and authority (userinfo/host/port), leaving the
/// remainder as the path.
///
/// TS: parseUri (`src/model/resourceid.ts`, private)
///
/// All delimiters this function scans for (`#`, `?`, `:`, `/`, `@`) are
/// single-byte ASCII, so splitting on UTF-8 byte offsets lands on the same
/// characters TS splits on by UTF-16 code unit (as in `model_util`'s ASCII
/// splits).
fn parse_uri(uri: &str) -> Result<UriComponents> {
    let mut s = uri;
    let mut fragment = None;
    let mut query = None;
    let mut protocol = None;
    let mut username = None;
    let mut password = None;
    let mut port = None;

    // fragment: split on the first '#'
    if let Some(hash_pos) = s.find('#') {
        let frag = &s[hash_pos + 1..];
        fragment = (!frag.is_empty()).then(|| frag.to_string());
        s = &s[..hash_pos];
    }

    // query: split on the first '?'
    if let Some(q_pos) = s.find('?') {
        query = Some(s[q_pos + 1..].to_string());
        s = &s[..q_pos];
    }

    // scheme: only recognised if the remainder does not start with '//'
    if !s.starts_with("//")
        && let Some(colon_pos) = s.find(':')
    {
        let candidate = &s[..colon_pos];
        if is_scheme(candidate) {
            protocol = Some(candidate.to_ascii_lowercase());
            s = &s[colon_pos + 1..];
        }
    }

    // authority: only present if the remainder starts with '//'
    if s.starts_with("//") {
        s = &s[2..];
        let (authority, rest) = match s.find('/') {
            Some(slash_pos) => (&s[..slash_pos], &s[slash_pos..]),
            None => (s, ""),
        };
        s = rest;
        let mut hostport = authority;
        if let Some(at_pos) = authority.find('@') {
            let userinfo = &authority[..at_pos];
            hostport = &authority[at_pos + 1..];
            if let Some(u_colon) = userinfo.find(':') {
                username = Some(userinfo[..u_colon].to_string());
                password = Some(userinfo[u_colon + 1..].to_string());
            } else {
                username = Some(userinfo.to_string());
            }
        }
        if let Some(p_colon) = hostport.rfind(':') {
            let maybe_port = &hostport[p_colon + 1..];
            if !maybe_port.is_empty() {
                if !maybe_port.bytes().all(|b| b.is_ascii_digit()) {
                    return Err(error(
                        ErrorKind::Error,
                        "resourceid-parseuri-invalidport",
                        Vec::new(),
                    ));
                }
                port = Some(maybe_port.to_string());
            }
        }
    }

    Ok(UriComponents {
        protocol,
        username,
        password,
        port,
        query,
        fragment,
        path: s.to_string(),
    })
}

/// `/^[a-z][a-z0-9.+-]*$/i`: a URI scheme, the leading segment of `uri` up
/// to (not including) its first `:`.
fn is_scheme(candidate: &str) -> bool {
    let mut chars = candidate.chars();
    match chars.next() {
        Some(c) if c.is_ascii_alphabetic() => {}
        _ => return false,
    }
    chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '+' | '-'))
}

/// `encodeURI`'s unescaped set beyond alphanumerics: the "mark" characters
/// and the reserved characters it leaves alone (only `%` itself, and the
/// characters not in either set, are percent-encoded).
const ENCODE_URI_UNESCAPED_EXTRA: &str = "-_.!~*'();,/?:@&=+$#";

/// `encodeURI(id)`: every byte outside the unescaped set becomes `%XX`
/// (uppercase hex, one triplet per UTF-8 byte, as V8 encodes non-ASCII
/// characters).
///
/// TS: `encodeURI` (built in), called from `ResourceId.prototype.toURI`
fn encode_uri(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for ch in input.chars() {
        if ch.is_ascii_alphanumeric() || ENCODE_URI_UNESCAPED_EXTRA.contains(ch) {
            out.push(ch);
        } else {
            let mut buf = [0u8; 4];
            for byte in ch.encode_utf8(&mut buf).as_bytes() {
                out.push('%');
                out.push_str(&format!("{byte:02X}"));
            }
        }
    }
    out
}

/// A malformed percent-escape, as `decodeURIComponent` raises `URIError:
/// URI malformed` in V8. Not a catalogue entry of its own: `ResourceId`'s
/// TS source never catches this (only `parseUri`'s errors are caught, in
/// `fromURI`), so a malformed escape is not a case any current fixture or
/// coupled test exercises, and no ported member's message catalogue entry
/// claims it. [`ContractError::pre_port`] carries the real V8 text without
/// overclaiming a verbatim catalogue port (PORTING.md section 7.2).
fn malformed_uri_error() -> ConcertoError {
    ContractError::pre_port(ErrorKind::Error, "URI malformed".to_string(), None).into()
}

/// `decodeURIComponent(id)`: every `%XX` triplet becomes the byte `XX`:
/// unlike `decodeURI`, every percent-escape is decoded, reserved characters
/// included. The decoded bytes are then read back as UTF-8, exactly
/// reversing [`encode_uri`].
///
/// TS: `decodeURIComponent` (built in), called from `ResourceId.fromURI`
fn decode_uri_component(input: &str) -> Result<String> {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            if i + 3 > bytes.len() {
                return Err(malformed_uri_error());
            }
            let hex =
                std::str::from_utf8(&bytes[i + 1..i + 3]).map_err(|_| malformed_uri_error())?;
            let byte = u8::from_str_radix(hex, 16).map_err(|_| malformed_uri_error())?;
            out.push(byte);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).map_err(|_| malformed_uri_error())
}

/// All the identifying properties of a resource: a namespace, a short type
/// name within it, and an instance identifier. Internal framework use only
/// (TS marks the whole class `@private`).
///
/// TS: ResourceId (`src/model/resourceid.ts`)
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResourceId {
    /// Namespace containing the type.
    pub namespace: String,
    /// Short type name.
    pub type_name: String,
    /// Instance identifier.
    pub id: String,
}

impl ResourceId {
    /// <strong>Note: only for use by internal framework code.</strong>
    ///
    /// TS: ResourceId constructor (`src/model/resourceid.ts`)
    ///
    /// ```
    /// # use concerto_core::instance::resource_id::ResourceId;
    /// let id = ResourceId::new("org.acme", "Person", "123").unwrap();
    /// assert_eq!(id.namespace, "org.acme");
    /// assert_eq!(id.type_name, "Person");
    /// assert_eq!(id.id, "123");
    /// assert!(ResourceId::new("", "Person", "123").is_err());
    /// ```
    pub fn new(
        namespace: impl Into<String>,
        type_name: impl Into<String>,
        id: impl Into<String>,
    ) -> Result<Self> {
        let namespace = namespace.into();
        let type_name = type_name.into();
        let id = id.into();
        if namespace.is_empty() {
            return Err(error(
                ErrorKind::Error,
                "resourceid-constructor-missingnamespace",
                Vec::new(),
            ));
        }
        if type_name.is_empty() {
            return Err(error(
                ErrorKind::Error,
                "resourceid-constructor-missingtype",
                Vec::new(),
            ));
        }
        if id.is_empty() {
            return Err(error(
                ErrorKind::Error,
                "resourceid-constructor-missingid",
                Vec::new(),
            ));
        }
        Ok(Self {
            namespace,
            type_name,
            id,
        })
    }

    /// Parse a URI into an identifier.
    ///
    /// Three formats are allowable:
    /// 1. Valid resource URI argument: `resource:qualifiedTypeName#ID`
    /// 2. Valid resource URI argument with missing URI scheme:
    ///    `qualifiedTypeName#ID`
    /// 3. URI argument containing only an ID, with legacy namespace and
    ///    type arguments supplied.
    ///
    /// TS: ResourceId.fromURI (`src/model/resourceid.ts`)
    ///
    /// ```
    /// # use concerto_core::instance::resource_id::ResourceId;
    /// let id = ResourceId::from_uri("resource:org.acme.Person#123", None, None).unwrap();
    /// assert_eq!(id.namespace, "org.acme");
    /// assert_eq!(id.type_name, "Person");
    /// assert_eq!(id.id, "123");
    ///
    /// let legacy = ResourceId::from_uri("123", Some("org.acme"), Some("Person")).unwrap();
    /// assert_eq!(legacy.namespace, "org.acme");
    /// ```
    pub fn from_uri(
        uri: &str,
        legacy_namespace: Option<&str>,
        legacy_type: Option<&str>,
    ) -> Result<Self> {
        let components = parse_uri(uri).map_err(|_| {
            error(
                ErrorKind::Error,
                "resourceid-fromuri-invaliduri",
                vec![("uri", uri.to_string())],
            )
        })?;

        // Accept legacy identifiers with missing URI scheme as valid
        if let Some(scheme) = &components.protocol
            && scheme != RESOURCE_SCHEME
        {
            return Err(error(
                ErrorKind::Error,
                "resourceid-fromuri-invalidscheme",
                vec![("uri", uri.to_string())],
            ));
        }
        if is_present(&components.username)
            || is_present(&components.password)
            || is_present(&components.port)
            || is_present(&components.query)
        {
            return Err(error(
                ErrorKind::Error,
                "resourceid-fromuri-invalidformat",
                vec![("uri", uri.to_string())],
            ));
        }

        let (namespace, type_name, raw_id) = match &components.fragment {
            Some(id) => {
                // The whole path is a qualified type name.
                let qualified_type = components.path.as_str();
                let namespace = model_util::get_namespace(Some(qualified_type))?.to_string();
                let type_name = model_util::get_short_name(qualified_type).to_string();
                (namespace, type_name, id.clone())
            }
            None => {
                // Legacy format where the whole path is the ID.
                (
                    legacy_namespace.unwrap_or_default().to_string(),
                    legacy_type.unwrap_or_default().to_string(),
                    components.path.clone(),
                )
            }
        };

        Self::new(namespace, type_name, decode_uri_component(&raw_id)?)
    }

    /// URI representation of this identifier.
    ///
    /// TS: ResourceId.prototype.toURI (`src/model/resourceid.ts`)
    pub fn to_uri(&self) -> String {
        let qualified_type = model_util::get_fully_qualified_name(&self.namespace, &self.type_name);
        format!(
            "{RESOURCE_SCHEME}:{qualified_type}#{}",
            encode_uri(&self.id)
        )
    }
}

#[cfg(test)]
mod tests {
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
    // which exercise ResourceId only through Relationship (no direct
    // resourceid test file exists in TS; SEAM_LEDGER.tsv's
    // coupled_tests_grep for `toURI` names this file).

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
        let id =
            ResourceId::from_uri("resource:org.acme.l1@1.0.0.Person#%CE%A9", None, None).unwrap();
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
        let err =
            ResourceId::from_uri("banana:org.acme.l1@1.0.0.Person#123", None, None).unwrap_err();
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
}
