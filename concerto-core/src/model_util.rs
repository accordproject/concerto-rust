//! Name, namespace and type helpers: a port of `ModelUtil`
//! (`src/modelutil.ts`) from the TypeScript reference.
//!
//! Concerto names a declaration with a fully-qualified name like
//! `namespace.ShortName`, and the namespace usually carries a `@version` (so
//! `org.example@1.0.0.Person`). The functions here split those apart and put
//! them back together, and answer what a name is allowed to be. Every function
//! follows its TS counterpart exactly, including the cases where that differs
//! from the Concerto v4 specification (PORTING.md 3.6).
//!
//! The functions that, in TS, call collaborator objects (`isAssignableTo`,
//! `isEnum`, `isMap`, `isScalar`, `isValidMapKeyScalar`) make those calls
//! through a `ResolutionContext`.

use std::sync::LazyLock;

#[cfg(feature = "js-compat")]
use crate::json::Value;

#[cfg(feature = "js-compat")]
use crate::ecma;
#[cfg(feature = "js-compat")]
use crate::error::ContractError;
use crate::error::{Error, ErrorKind, Result};
use crate::model_manager::ResolutionContext;

/// `ID_REGEX` from `src/modelutil.ts`, character for character
/// (PORTING.md 3.4), compiled with the `u` flag.
const ID_PATTERN: &str = r"^(\p{Lu}|\p{Ll}|\p{Lt}|\p{Lm}|\p{Lo}|\p{Nl}|\$|_|\\u[0-9A-Fa-f]{4})(?:\p{Lu}|\p{Ll}|\p{Lt}|\p{Lm}|\p{Lo}|\p{Nl}|\$|_|\\u[0-9A-Fa-f]{4}|\p{Mn}|\p{Mc}|\p{Nd}|\p{Pc}|‌|‍)*$";

static ID_REGEX: LazyLock<regress::Regex> = LazyLock::new(|| {
    // A constant pattern; `id_regex_compiles` tests that it compiles.
    regress::Regex::with_flags(ID_PATTERN, "u").expect("ID_REGEX is a valid ECMAScript pattern")
});

/// The metamodel namespace (`MetaModelNamespace` in `concerto-metamodel`).
#[cfg(feature = "js-compat")]
const METAMODEL_NAMESPACE: &str = "concerto.metamodel@1.0.0";

/// `primitiveTypes` in `ModelUtil.isPrimitiveType`, in TS order.
pub(crate) const PRIMITIVE_TYPES: &[&str] =
    &["Boolean", "String", "DateTime", "Double", "Integer", "Long"];

/// `privateReservedProperties` in `src/modelutil.ts`, in TS order.
const PRIVATE_RESERVED_PROPERTIES: &[&str] = &[
    // Internal use only
    "$classDeclaration",
    "$namespace",
    "$type",
    "$modelManager",
    "$validator",
    "$identifierFieldName",
    "$imports",
    "$superTypes",
    // Included in serialization
    "$id",
];

/// `assignableReservedProperties` in `src/modelutil.ts`.
const ASSIGNABLE_RESERVED_PROPERTIES: &[&str] = &["$identifier", "$timestamp"];

/// The short name: whatever comes after the last `.`.
///
/// ```
/// # use concerto_core::model_util::short_name;
/// assert_eq!(short_name("org.example@1.0.0.Person"), "Person");
/// assert_eq!(short_name("Person"), "Person");
/// ```
pub fn short_name(fqn: &str) -> &str {
    // `lastIndexOf('.')` + `substr(i + 1)` count UTF-16 units. '.' is one
    // UTF-16 unit and one UTF-8 byte, so the byte split gives the same string.
    match fqn.rfind('.') {
        Some(dot) => &fqn[dot + 1..],
        None => fqn,
    }
}

/// The namespace: everything before the last `.`, or the empty string if the
/// name is not qualified.
///
/// ```
/// # use concerto_core::model_util::namespace_of;
/// assert_eq!(namespace_of("org.example@1.0.0.Person"), "org.example@1.0.0");
/// assert_eq!(namespace_of("Person"), "");
/// ```
pub fn namespace_of(fqn: &str) -> &str {
    // As in `short_name`, the split point is the ASCII '.'.
    match fqn.rfind('.') {
        Some(dot) => &fqn[..dot],
        None => "",
    }
}

/// Puts a namespace and a short name back together. An empty namespace gives
/// the short name back unchanged, which is what a primitive type needs.
///
/// ```
/// # use concerto_core::model_util::qualify;
/// assert_eq!(qualify("org.example@1.0.0", "Person"), "org.example@1.0.0.Person");
/// assert_eq!(qualify("", "String"), "String");
/// ```
pub fn qualify(namespace: &str, short: &str) -> String {
    if namespace.is_empty() {
        short.to_string()
    } else {
        format!("{namespace}.{short}")
    }
}

/// Returns everything after the last dot, if present, of the source string:
/// the TS name of [`short_name`].
///
/// TS: ModelUtil.getShortName (src/modelutil.ts)
///
/// ```
/// # #![allow(deprecated)]
/// # use concerto_core::model_util::get_short_name;
/// assert_eq!(get_short_name("org.acme.baz@1.0.0.Foo"), "Foo");
/// assert_eq!(get_short_name("Foo"), "Foo");
/// ```
#[deprecated(since = "0.1.0", note = "use `short_name`")]
pub fn get_short_name(fqn: &str) -> &str {
    short_name(fqn)
}

js_compat_pub! {
    /// Returns the namespace of a fully qualified name: everything before the
    /// last dot, or the empty string if there is no dot. `None` (JS `undefined` or
    /// `null`) and `""` fail the TS `!fqn` check.
    ///
    /// TS: ModelUtil.getNamespace (src/modelutil.ts)
    ///
    /// ```
    /// # use concerto_core::model_util::get_namespace;
    /// assert_eq!(get_namespace(Some("org.acme.baz@1.0.0.Foo")).unwrap(), "org.acme.baz@1.0.0");
    /// assert_eq!(get_namespace(Some("Foo")).unwrap(), "");
    /// assert!(get_namespace(None).is_err());
    /// ```
    pub fn get_namespace(fqn: Option<&str>) -> Result<&str> {
        let fqn = match fqn {
            Some(fqn) if !fqn.is_empty() => fqn,
            _ => {
                return Err(Error::new(
                    ErrorKind::InvalidArgument,
                    "modelutil-getnamespace-nofnq",
                    Vec::new(),
                ));
            }
        };
        Ok(namespace_of(fqn))
    }
}

/// A prerelease identifier of a [`SemVer`]: numeric identifiers below
/// `Number.MAX_SAFE_INTEGER` become numbers, as node-semver does.
#[derive(Debug, Clone, PartialEq)]
pub enum PrereleaseIdentifier {
    /// A numeric identifier.
    Number(f64),
    /// Any other identifier.
    String(String),
}

/// A parsed namespace version, in the shape `semver.parse` (node-semver
/// 7.6.3, the version concerto-core 5.0.0 resolves) returns: see
/// `semver_parse` for the strict SemVer 2.0.0 grammar (BC-41).
#[derive(Debug, Clone, PartialEq)]
pub struct SemVer {
    /// The version as given (`raw`).
    pub raw: String,
    /// `major`.
    pub major: f64,
    /// `minor`.
    pub minor: f64,
    /// `patch`.
    pub patch: f64,
    /// `prerelease`.
    pub prerelease: Vec<PrereleaseIdentifier>,
    /// `build`.
    pub build: Vec<String>,
    /// `version`: `major.minor.patch`, plus `-prerelease` when there is one.
    pub version: String,
}

/// `Number.MAX_SAFE_INTEGER`, the largest prerelease identifier that
/// becomes a [`PrereleaseIdentifier::Number`].
const MAX_SAFE_INTEGER: f64 = 9_007_199_254_740_991.0;

/// `Number.MAX_SAFE_INTEGER`, node-semver's bound on major, minor and patch.
const MAX_SAFE_INTEGER_U64: u64 = 9_007_199_254_740_991;

/// node-semver's `MAX_LENGTH`, in UTF-16 units.
const SEMVER_MAX_LENGTH: usize = 256;

/// Whether `version` is a valid namespace or import version: strict SemVer
/// 2.0.0 (BC-41), the `semver` crate's `semver::Version::parse` with no
/// node-compat leniency. Surrounding whitespace and a leading `v` are
/// rejected, as the CTO grammar already rejects them, and major, minor and
/// patch go up to `u64::MAX` (2^64-1).
pub(crate) fn is_strict_semver(version: &str) -> bool {
    semver::Version::parse(version).is_ok()
}

/// node-semver 7.6.3's `parse` restricted to strict SemVer 2.0.0
/// ([`is_strict_semver`]): TS `parseNamespace`'s `versionParsed`. `None` also
/// for a strict version beyond node-semver's limits (a component above
/// `Number.MAX_SAFE_INTEGER`, or over 256 UTF-16 units), which is still
/// valid (BC-41). Checked by `tests/semver/node-semver-7.6.3.json`.
pub(crate) fn semver_parse(version: &str) -> Option<SemVer> {
    // A string has no more UTF-16 units than UTF-8 bytes.
    if version.len() > SEMVER_MAX_LENGTH && version.encode_utf16().count() > SEMVER_MAX_LENGTH {
        return None;
    }
    let parsed = semver::Version::parse(version).ok()?;
    if parsed.major > MAX_SAFE_INTEGER_U64
        || parsed.minor > MAX_SAFE_INTEGER_U64
        || parsed.patch > MAX_SAFE_INTEGER_U64
    {
        return None;
    }
    // `+m[i]`: every component is at most MAX_SAFE_INTEGER, so the `u64` is
    // exactly the JS number.
    #[allow(clippy::cast_precision_loss)]
    let (major, minor, patch) = (
        parsed.major as f64,
        parsed.minor as f64,
        parsed.patch as f64,
    );
    let prerelease: Vec<PrereleaseIdentifier> = if parsed.pre.is_empty() {
        Vec::new()
    } else {
        parsed
            .pre
            .as_str()
            .split('.')
            .map(|id| {
                if !id.is_empty() && id.bytes().all(|b| b.is_ascii_digit()) {
                    let num = id.parse::<f64>().unwrap_or(f64::NAN);
                    if (0.0..MAX_SAFE_INTEGER).contains(&num) {
                        return PrereleaseIdentifier::Number(num);
                    }
                }
                PrereleaseIdentifier::String(id.to_string())
            })
            .collect()
    };
    let build = if parsed.build.is_empty() {
        Vec::new()
    } else {
        parsed
            .build
            .as_str()
            .split('.')
            .map(str::to_string)
            .collect()
    };
    // `format()`: `major.minor.patch`, plus `-` and the prerelease
    // identifiers joined with `.`. Every component has no leading zero and
    // is at most MAX_SAFE_INTEGER, and every prerelease identifier that
    // became a number is below it, so `String(n)` gives back its digits.
    let mut formatted = format!("{}.{}.{}", parsed.major, parsed.minor, parsed.patch);
    if !parsed.pre.is_empty() {
        formatted.push('-');
        formatted.push_str(parsed.pre.as_str());
    }
    Some(SemVer {
        raw: version.to_string(),
        major,
        minor,
        patch,
        prerelease,
        build,
        version: formatted,
    })
}

/// `parse_namespace_with(Some(ns), false)`'s checks and errors, in its order,
/// returning `name` and `version` borrowed from `ns` (the model load and
/// `ModelFile.validate()`'s import loop read only these). An unversioned
/// namespace is not an error here (`version` is `None`, unlike BC-02's
/// `parse_namespace_with`): these callers reject it with their own errors.
pub(crate) fn split_namespace(ns: &str) -> Result<(&str, Option<&str>)> {
    if ns.is_empty() {
        return Err(Error::new(
            ErrorKind::InvalidArgument,
            "modelutil-parsenamespace-nullorundefined",
            Vec::new(),
        ));
    }
    let invalid = || {
        Error::new(
            ErrorKind::InvalidArgument,
            "modelutil-parsenamespace-invalidnamespace",
            vec![("ns", ns.to_string())],
        )
    };
    let mut parts = ns.split('@');
    let name = parts.next().unwrap_or_default();
    let version = parts.next();
    if parts.next().is_some() {
        return Err(invalid());
    }
    // BC-41: acceptance is strict SemVer 2.0.0, as in
    // `parse_namespace_with`; `versionParsed` is not built
    // here.
    if let Some(version) = version
        && !is_strict_semver(version)
    {
        return Err(invalid());
    }
    Ok((name, version))
}

/// The result of [`parse_namespace`]: the TS `ParseNamespaceResult`.
#[derive(Debug, Clone, PartialEq)]
pub enum ParsedNamespace {
    /// With `disableVersionParsing`, TS returns `{ name }` only.
    NameOnly {
        /// The namespace without its version.
        name: String,
    },
    /// Otherwise all four keys.
    Full {
        /// `name`: the namespace without its version.
        name: String,
        /// `escapedNamespace`: the namespace with its first `@` replaced by `_`.
        escaped_namespace: String,
        /// `version`: the text after the `@`, or `None` (JS `null`).
        version: Option<String>,
        /// `versionParsed`: the parsed version, or `None` (JS `null`): with
        /// no version, or with a version beyond node-semver's own limits
        /// (a component above 2^53-1, or more than 256 UTF-16 units; see
        /// `semver_parse`), which is still accepted (BC-41).
        version_parsed: Option<SemVer>,
    },
}

/// `parse_namespace_with(Some(ns), false)`: the same checks and errors, in
/// its order, with its `name` and `version` borrowed from `ns`. A namespace
/// that parses always has a version (BC-02).
#[cfg(feature = "js-compat")]
pub(crate) fn namespace_parts(ns: &str) -> Result<(&str, &str)> {
    let (name, version) = split_namespace(ns)?;
    let version = version.ok_or_else(|| {
        Error::new(
            ErrorKind::InvalidArgument,
            "modelutil-parsenamespace-invalidnamespace",
            vec![("ns", ns.to_string())],
        )
    })?;
    Ok((name, version))
}

/// Parses a namespace into its name and its version. An unversioned or
/// empty namespace is an error (BC-02).
///
/// ```
/// # use concerto_core::model_util::{parse_namespace, ParsedNamespace};
/// let ParsedNamespace::Full { name, version, .. } = parse_namespace("org.acme@1.0.0").unwrap() else {
///     unreachable!()
/// };
/// assert_eq!((name.as_str(), version.as_deref()), ("org.acme", Some("1.0.0")));
/// assert!(parse_namespace("org.acme@1.0.0@2.3").is_err());
/// assert!(parse_namespace("org.acme").is_err());
/// ```
pub fn parse_namespace(ns: &str) -> Result<ParsedNamespace> {
    parse_namespace_with(Some(ns), false)
}

js_compat_pub! {
    /// [`parse_namespace`], in the shape of TS `ModelUtil.parseNamespace(ns,
    /// disableVersionParsing)`. `None` (JS `undefined` or `null`) and `""` fail
    /// the TS `!ns` check. An unversioned namespace is rejected, with or
    /// without `disable_version_parsing`, as Concerto v4 requires (BC-02;
    /// TS 5.0.0 accepted it with `version: null`).
    ///
    /// TS: ModelUtil.parseNamespace (src/modelutil.ts)
    ///
    /// ```
    /// # use concerto_core::model_util::{parse_namespace_with, ParsedNamespace};
    /// let ParsedNamespace::Full { name, version, .. } = parse_namespace_with(Some("org.acme@1.0.0"), false).unwrap() else {
    ///     unreachable!()
    /// };
    /// assert_eq!((name.as_str(), version.as_deref()), ("org.acme", Some("1.0.0")));
    /// assert!(parse_namespace_with(Some("org.acme@1.0.0@2.3"), false).is_err());
    /// assert!(parse_namespace_with(Some("org.acme"), false).is_err());
    /// assert!(parse_namespace_with(Some("org.acme"), true).is_err());
    /// ```
    pub fn parse_namespace_with(ns: Option<&str>, disable_version_parsing: bool) -> Result<ParsedNamespace> {
        let ns = match ns {
            Some(ns) if !ns.is_empty() => ns,
            _ => {
                return Err(Error::new(
                    ErrorKind::InvalidArgument,
                    "modelutil-parsenamespace-nullorundefined",
                    Vec::new(),
                ));
            }
        };
        let invalid = || {
            Error::new(
                ErrorKind::InvalidArgument,
                "modelutil-parsenamespace-invalidnamespace",
                vec![("ns", ns.to_string())],
            )
        };
        let parts: Vec<&str> = ns.split('@').collect();
        // BC-02: a namespace must carry a version, whether or not the
        // version is parsed, rejected with the error an invalid one gets.
        if parts.len() != 2 {
            return Err(invalid());
        }
        let mut version_parsed = None;
        if let [_, version] = parts.as_slice()
            && !disable_version_parsing
        {
            // BC-41: strict SemVer 2.0.0 is accepted; `versionParsed` is
            // node-semver's `parse`, `null` beyond its limits, as in TS.
            if !is_strict_semver(version) {
                return Err(invalid());
            }
            version_parsed = semver_parse(version);
        }
        let name = parts.first().copied().unwrap_or_default().to_string();
        if disable_version_parsing {
            return Ok(ParsedNamespace::NameOnly { name });
        }
        Ok(ParsedNamespace::Full {
            name,
            // `String.prototype.replace` with a string pattern replaces the first
            // occurrence only.
            escaped_namespace: ns.replacen('@', "_", 1),
            version: parts.get(1).map(|v| (*v).to_string()),
            version_parsed,
        })
    }
}

/// Returns the fully qualified names an import brings in. `imp` is the
/// import's AST node (`None` is JS `undefined`). The TS member delegates to
/// `MetaModelUtil.importFullyQualifiedNames` (concerto-metamodel 3.17.0), which
/// is ported here with it.
///
/// TS: ModelUtil.importFullyQualifiedNames (src/modelutil.ts)
#[cfg(feature = "js-compat")]
pub fn import_fully_qualified_names(imp: Option<&Value>) -> Result<Vec<String>> {
    let class = class_of(imp)?;
    let imp = imp.unwrap_or(&Value::Null);
    let field = |name: &str| {
        imp.get(name)
            .map_or_else(|| "undefined".to_string(), ecma::to_js_string)
    };
    let is = |short: &str| class == Some(format!("{METAMODEL_NAMESPACE}.{short}").as_str());
    if is("ImportAll") {
        Ok(vec![format!("{}.*", field("namespace"))])
    } else if is("ImportType") {
        Ok(vec![format!("{}.{}", field("namespace"), field("name"))])
    } else if is("ImportTypes") {
        match imp.get("types") {
            Some(Value::Array(types)) => Ok(types
                .iter()
                .map(|t| format!("{}.{}", field("namespace"), ecma::to_js_string(t)))
                .collect()),
            None | Some(Value::Null) => Err(Error::new(
                ErrorKind::MalformedInput,
                "engine-typeerror-readproperties",
                vec![
                    ("value", field("types")),
                    ("property", "forEach".to_string()),
                ],
            )),
            Some(_) => Err(Error::new(
                ErrorKind::MalformedInput,
                "engine-typeerror-notafunction",
                vec![("expression", "imp.types.forEach".to_string())],
            )),
        }
    } else {
        Err(Error::new(
            ErrorKind::InvalidArgument,
            "metamodelutil-importfullyqualifiednames-unrecognizedimports",
            vec![("$class", field("$class"))],
        ))
    }
}

/// Returns true if the type is one of the six primitive types.
///
/// TS: ModelUtil.isPrimitiveType (src/modelutil.ts)
///
/// ```
/// # use concerto_core::model_util::is_primitive_type;
/// assert!(is_primitive_type("String"));
/// assert!(!is_primitive_type("org.acme.baz@1.0.0.Foo"));
/// ```
pub fn is_primitive_type(type_name: &str) -> bool {
    PRIMITIVE_TYPES.contains(&type_name)
}

/// Returns true if a value of type `type_name` can be assigned to `property`.
/// A primitive on either side must match exactly; otherwise the type is looked
/// up in `model_file` and its super types are searched.
///
/// TS: ModelUtil.isAssignableTo (src/modelutil.ts)
#[cfg(feature = "js-compat")]
pub fn is_assignable_to<C: ResolutionContext>(
    ctx: &C,
    model_file: &C::Node,
    type_name: &str,
    property: &C::Node,
) -> std::result::Result<bool, C::Error> {
    let property_type_name = ctx.get_fully_qualified_type_name(property)?;
    is_assignable_to_type(ctx, model_file, type_name, &property_type_name)
}

/// [`is_assignable_to`] for a property whose fully qualified type name
/// the caller has already read (`property.getFullyQualifiedTypeName()`):
/// the WASM binding reads it from the JS property, which may be a
/// stand-in such as a relationship map value (BC-52).
///
/// TS: ModelUtil.isAssignableTo (src/modelutil.ts), after its first line
#[cfg(feature = "js-compat")]
pub fn is_assignable_to_type<C: ResolutionContext>(
    ctx: &C,
    model_file: &C::Node,
    type_name: &str,
    property_type_name: &str,
) -> std::result::Result<bool, C::Error> {
    let is_direct_match = type_name == property_type_name;
    if is_direct_match || is_primitive_type(type_name) || is_primitive_type(property_type_name) {
        return Ok(is_direct_match);
    }

    let Some(type_declaration) = ctx.get_type(model_file, Some(type_name))? else {
        return Err(ContractError::new(
            ErrorKind::InvalidArgument,
            "modelutil-isassignableto-cannotfindtype",
            vec![("typeName", type_name.to_string())],
        )
        .into());
    };

    // `.some(...)` stops at the first match, so the names are read lazily.
    for super_type in ctx.get_all_super_type_declarations(&type_declaration)? {
        if ctx.get_fully_qualified_name(&super_type)? == property_type_name {
            return Ok(true);
        }
    }
    Ok(false)
}

js_compat_pub! {
    /// Returns the string with its first character (one UTF-16 code unit)
    /// upper-cased.
    ///
    /// TS: ModelUtil.capitalizeFirstLetter (src/modelutil.ts)
    ///
    /// ```
    /// # use concerto_core::model_util::capitalize_first_letter;
    /// assert_eq!(capitalize_first_letter("aBcDeF"), "ABcDeF");
    /// ```
    pub fn capitalize_first_letter(s: &str) -> String {
        let mut chars = s.chars();
        match chars.next() {
            None => String::new(),
            // A code point outside the BMP is a surrogate pair in JS: `charAt(0)`
            // takes the lone high surrogate, which `toUpperCase` leaves alone, so
            // the string comes back unchanged.
            Some(first) if first.len_utf16() > 1 => s.to_string(),
            // `toUpperCase` uses the full (unconditional SpecialCasing) mapping,
            // as `char::to_uppercase` does: 'ß' becomes "SS".
            Some(first) => {
                let mut out: String = first.to_uppercase().collect();
                out.push_str(chars.as_str());
                out
            }
        }
    }
}

/// Resolves the type of `field` in its parent's model file: the shared first
/// half of `isEnum`, `isMap` and `isScalar`.
fn field_type_declaration<C: ResolutionContext>(
    ctx: &C,
    field: &C::Node,
) -> std::result::Result<Option<C::Node>, C::Error> {
    let parent = ctx.get_parent(field)?;
    let model_file = ctx.get_model_file(&parent)?;
    let type_name = ctx.get_type_name(field)?;
    ctx.get_type(&model_file, type_name.as_deref())
}

js_compat_pub! {
    /// Returns whether the field's type is an enum, or `None` (JS `undefined`)
    /// when the type is not found.
    ///
    /// TS: ModelUtil.isEnum (src/modelutil.ts)
    pub fn is_enum<C: ResolutionContext>(
        ctx: &C,
        field: &C::Node,
    ) -> std::result::Result<Option<bool>, C::Error> {
        match field_type_declaration(ctx, field)? {
            Some(declaration) => Ok(Some(ctx.is_enum(&declaration)?)),
            None => Ok(None),
        }
    }
}

/// Returns whether the field's type is a map, or `None` (JS `undefined`) when
/// the type is not found or has no `isMapDeclaration` method.
///
/// TS: ModelUtil.isMap (src/modelutil.ts)
#[cfg(feature = "js-compat")]
pub fn is_map<C: ResolutionContext>(
    ctx: &C,
    field: &C::Node,
) -> std::result::Result<Option<bool>, C::Error> {
    match field_type_declaration(ctx, field)? {
        Some(declaration) => ctx.is_map_declaration(&declaration),
        None => Ok(None),
    }
}

/// Returns whether the field's type is a scalar, or `None` (JS `undefined`)
/// when the type is not found or has no `isScalarDeclaration` method.
///
/// TS: ModelUtil.isScalar (src/modelutil.ts)
#[cfg(feature = "js-compat")]
pub fn is_scalar<C: ResolutionContext>(
    ctx: &C,
    field: &C::Node,
) -> std::result::Result<Option<bool>, C::Error> {
    match field_type_declaration(ctx, field)? {
        Some(declaration) => ctx.is_scalar_declaration(&declaration),
        None => Ok(None),
    }
}

/// Returns true if the name is a valid Concerto identifier: `ID_REGEX.test`.
/// A JS caller's non-string (`undefined`, `null`, ...) is not a valid
/// identifier: the binding answers `false` without calling this (BC-01;
/// TS 5.0.0 tested `String(name)`, so `"undefined"` and `"null"` passed,
/// DV-002).
///
/// TS: ModelUtil.isValidIdentifier (src/modelutil.ts)
///
/// ```
/// # use concerto_core::model_util::is_valid_identifier;
/// assert!(is_valid_identifier("suchName"));
/// assert!(!is_valid_identifier("1st"));
/// ```
pub fn is_valid_identifier(name: &str) -> bool {
    // Fast path: a non-empty ASCII `[A-Za-z$_][A-Za-z0-9$_]*` name is
    // always a match (ASCII letters are `\p{Lu}`/`\p{Ll}`, ASCII digits
    // `\p{Nd}`, and `$`/`_` are listed), so only names outside that subset
    // pay for the regex. The subset only ever answers `true`, so the regex
    // alone still decides every rejection.
    let bytes = name.as_bytes();
    if let Some((first, rest)) = bytes.split_first()
        && (first.is_ascii_alphabetic() || *first == b'$' || *first == b'_')
        && rest
            .iter()
            .all(|b| b.is_ascii_alphanumeric() || *b == b'$' || *b == b'_')
    {
        return true;
    }
    ID_REGEX.find(name).is_some()
}

/// Returns the fully qualified name of a type: `namespace.type`, or `type`
/// alone when the namespace is empty (falsy in TS). The TS name of
/// [`qualify`].
///
/// TS: ModelUtil.getFullyQualifiedName (src/modelutil.ts)
///
/// ```
/// # #![allow(deprecated)]
/// # use concerto_core::model_util::get_fully_qualified_name;
/// assert_eq!(get_fully_qualified_name("a.namespace", "type"), "a.namespace.type");
/// assert_eq!(get_fully_qualified_name("", "type"), "type");
/// ```
#[deprecated(since = "0.1.0", note = "use `qualify`")]
pub fn get_fully_qualified_name(namespace: &str, type_name: &str) -> String {
    qualify(namespace, type_name)
}

/// Removes the namespace version from a fully qualified name. Primitive types
/// are returned unchanged. `None` (JS `undefined` or `null`) fails like `""`.
///
/// TS: ModelUtil.removeNamespaceVersionFromFullyQualifiedName (src/modelutil.ts)
///
/// ```
/// # use concerto_core::model_util::remove_namespace_version_from_fully_qualified_name as remove;
/// assert_eq!(remove(Some("org.acme@1.0.0.Person")).unwrap(), "org.acme.Person");
/// assert_eq!(remove(Some("String")).unwrap(), "String");
/// ```
#[cfg(feature = "js-compat")]
pub fn remove_namespace_version_from_fully_qualified_name(fqn: Option<&str>) -> Result<String> {
    if let Some(fqn) = fqn
        && is_primitive_type(fqn)
    {
        return Ok(fqn.to_string());
    }
    let ns = get_namespace(fqn)?;
    let (namespace, _) = namespace_parts(ns)?;
    // `get_namespace` succeeded, so `fqn` is a non-empty string.
    let type_name = short_name(fqn.unwrap_or_default());
    Ok(qualify(namespace, type_name))
}

/// Returns true if the property name is reserved by Concerto.
///
/// TS: ModelUtil.isSystemProperty (src/modelutil.ts)
///
/// ```
/// # use concerto_core::model_util::is_system_property;
/// assert!(is_system_property("$class"));
/// assert!(!is_system_property("$numberOfWipers"));
/// ```
pub fn is_system_property(property_name: &str) -> bool {
    property_name == "$class"
        || ASSIGNABLE_RESERVED_PROPERTIES.contains(&property_name)
        || PRIVATE_RESERVED_PROPERTIES.contains(&property_name)
}

/// Returns true if the property name is reserved for Concerto's internal use.
///
/// TS: ModelUtil.isPrivateSystemProperty (src/modelutil.ts)
pub fn is_private_system_property(property_name: &str) -> bool {
    PRIVATE_RESERVED_PROPERTIES.contains(&property_name)
}

/// `node.$class` for an AST node, where `None` is JS `undefined`. Reading a
/// property of `undefined` or `null` is a JS `TypeError`.
#[cfg(feature = "js-compat")]
fn class_of(node: Option<&Value>) -> Result<Option<&str>> {
    match node {
        None | Some(Value::Null) => Err(Error::new(
            ErrorKind::MalformedInput,
            "engine-typeerror-readproperties",
            vec![
                (
                    "value",
                    if node.is_none() { "undefined" } else { "null" }.to_string(),
                ),
                ("property", "$class".to_string()),
            ],
        )),
        Some(node) => Ok(node.get("$class").and_then(Value::as_str)),
    }
}

/// The key kinds the specification allows: a `String` or `DateTime`, or an
/// object key naming a scalar over one of those.
///
/// The single source for this list: `ModelUtil.isValidMapKey`,
/// `validation::validate_map_key` and `introspect::declaration`'s
/// `MM_MAP_KEY_KINDS` check all read it.
#[cfg(feature = "js-compat")]
pub const MAP_KEY_KINDS: &[&str] = &["StringMapKeyType", "DateTimeMapKeyType", "ObjectMapKeyType"];

/// The value kinds the specification allows: any primitive, or an object or
/// relationship value naming a declared type.
///
/// TS: `ModelUtil.isValidMapValue` (src/modelutil.ts) lists the same eight
/// kinds, including `RelationshipMapValueType`. The single source for this
/// list, for the same reason as [`MAP_KEY_KINDS`].
#[cfg(feature = "js-compat")]
pub const MAP_VALUE_KINDS: &[&str] = &[
    "BooleanMapValueType",
    "DateTimeMapValueType",
    "DoubleMapValueType",
    "IntegerMapValueType",
    "LongMapValueType",
    "StringMapValueType",
    "ObjectMapValueType",
    "RelationshipMapValueType",
];

/// Returns true if the map key AST node is a valid map key type.
///
/// TS: ModelUtil.isValidMapKey (src/modelutil.ts)
#[cfg(feature = "js-compat")]
pub fn is_valid_map_key(key: Option<&Value>) -> Result<bool> {
    let class = class_of(key)?;
    Ok(MAP_KEY_KINDS
        .iter()
        .any(|short| class == Some(format!("{METAMODEL_NAMESPACE}.{short}").as_str())))
}

/// Returns whether the declaration is a String or DateTime scalar, keeping the
/// JS result of `a && b || c && d`: `None` (JS `undefined`) when `decl` is
/// absent or has no `isScalarDeclaration` method.
///
/// TS: ModelUtil.isValidMapKeyScalar (src/modelutil.ts)
#[cfg(feature = "js-compat")]
pub fn is_valid_map_key_scalar<C: ResolutionContext>(
    ctx: &C,
    decl: Option<&C::Node>,
) -> std::result::Result<Option<bool>, C::Error> {
    // `decl?.isScalarDeclaration?.() && decl?.ast.$class === <scalar>`; each
    // side of the `||` calls `isScalarDeclaration` again, as TS does.
    let side = |scalar: &str| -> std::result::Result<Option<bool>, C::Error> {
        let Some(decl) = decl else {
            return Ok(None);
        };
        match ctx.is_scalar_declaration(decl)? {
            Some(true) => {
                let class = ctx.get_ast_class(decl)?;
                Ok(Some(
                    class.as_deref() == Some(format!("{METAMODEL_NAMESPACE}.{scalar}").as_str()),
                ))
            }
            falsy => Ok(falsy),
        }
    };
    match side("StringScalar")? {
        Some(true) => Ok(Some(true)),
        _ => side("DateTimeScalar"),
    }
}

/// Returns true if the map value AST node is a valid map value type.
///
/// TS: ModelUtil.isValidMapValue (src/modelutil.ts)
#[cfg(feature = "js-compat")]
pub fn is_valid_map_value(value: Option<&Value>) -> Result<bool> {
    let class = class_of(value)?;
    Ok(MAP_VALUE_KINDS
        .iter()
        .any(|short| class == Some(format!("{METAMODEL_NAMESPACE}.{short}").as_str())))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn id_regex_compiles() {
        assert!(regress::Regex::with_flags(ID_PATTERN, "u").is_ok());
    }

    #[test]
    fn is_valid_identifier_fast_path_agrees_with_the_regex() {
        // Every answer the ASCII fast path gives is the regex's own.
        let alphabet: Vec<char> = (0x20u8..0x7f).map(char::from).collect();
        let mut names = vec![String::new()];
        for first in &alphabet {
            names.push(first.to_string());
            for second in ['a', 'Z', '0', '9', '$', '_', '-', ' ', '.', '\\', '\u{e9}'] {
                names.push(format!("{first}{second}"));
                names.push(format!("{first}{second}x"));
            }
        }
        names.extend([
            r"\u0041bc".to_string(),
            "a\u{0663}".to_string(),
            "\u{e9}t\u{e9}".to_string(),
        ]);
        for name in names {
            assert_eq!(
                is_valid_identifier(&name),
                ID_REGEX.find(&name).is_some(),
                "{name:?}"
            );
        }
    }

    /// The recording `tests/semver/record.mjs` made of node-semver 7.6.3's
    /// `parse`: `{semver, full, cases: [{input, parsed}]}`.
    fn node_semver_recording() -> Value {
        serde_json::from_str(include_str!("../tests/semver/node-semver-7.6.3.json"))
            .unwrap_or_else(|e| panic!("node-semver-7.6.3.json: {e}"))
    }

    /// [`split_namespace`] accepts and rejects exactly what
    /// `parse_namespace_with(_, false)` does, with the same error, and gives
    /// back its `name` and `version`; over every recorded semver input as a
    /// namespace version, and the other namespace shapes. The one exception
    /// (BC-02): an unversioned namespace, which `split_namespace` gives back
    /// with no version for its callers to reject.
    #[test]
    fn split_namespace_matches_parse_namespace() {
        let recording = node_semver_recording();
        let mut namespaces: Vec<String> = recording["cases"]
            .as_array()
            .unwrap_or_else(|| unreachable!())
            .iter()
            .map(|case| {
                let input = case["input"].as_str().unwrap_or_else(|| unreachable!());
                format!("org.acme@{input}")
            })
            .collect();
        namespaces.extend(
            [
                "",
                "org.acme",
                "org.acme@",
                "@1.0.0",
                "org.acme@1.0.0@2.0.0",
                "a@b@c",
                "concerto@1.0.0",
                "concerto",
                "org.acme@v1.0.0",
                "org.acme@ 1.0.0 ",
                "org.acme@1.0.0-beta.1+build.2",
                "org.acme@01.0.0",
            ]
            .map(str::to_string),
        );
        for ns in &namespaces {
            let split = split_namespace(ns);
            if !ns.is_empty() && !ns.contains('@') {
                assert!(parse_namespace_with(Some(ns), false).is_err(), "{ns:?}");
                assert_eq!(split.ok(), Some((ns.as_str(), None)), "{ns:?}");
                continue;
            }
            match parse_namespace_with(Some(ns), false) {
                Ok(ParsedNamespace::Full { name, version, .. }) => {
                    let (n, v) = split.unwrap_or_else(|e| panic!("{ns:?}: {e}"));
                    assert_eq!((n, v), (name.as_str(), version.as_deref()), "{ns:?}");
                }
                Ok(other) => panic!("{ns:?}: {other:?}"),
                Err(expected) => {
                    let actual = split.err().unwrap_or_else(|| panic!("{ns:?} accepted"));
                    assert_eq!(actual.to_string(), expected.to_string(), "{ns:?}");
                }
            }
        }
    }

    #[test]
    fn semver_recording_is_node_semver_7_6_3() {
        let recording = node_semver_recording();
        assert_eq!(recording["semver"], "7.6.3");
    }

    /// Whether a strict SemVer 2.0.0 version is beyond node-semver 7.6.3's
    /// own limits (BC-41): more than `MAX_LENGTH` (256) UTF-16 units, or a
    /// component above `Number.MAX_SAFE_INTEGER`. (Its regex's `{0,256}` and
    /// `{0,250}` identifier bounds cannot bind within 256 units.)
    fn beyond_node_semver_limits(input: &str) -> bool {
        let Ok(v) = semver::Version::parse(input) else {
            return false;
        };
        input.encode_utf16().count() > 256
            || [v.major, v.minor, v.patch]
                .iter()
                .any(|n| *n > 9_007_199_254_740_991)
    }

    #[test]
    fn semver_parse_matches_node_semver() {
        // The differential test against node-semver's own `parse`, over
        // prerelease and build metadata, leading zeros, whitespace, `v`
        // prefixes, numeric limits, very long input and random near-versions
        // (tests/semver/record.mjs). Since BC-41 `semver_parse` is
        // node-semver's result for an input with no surrounding whitespace
        // and no leading `v`, and nothing else; and a namespace version is
        // accepted ([`is_strict_semver`]) exactly when it gets a result or
        // only node-semver's own limits reject it (then `versionParsed` is
        // null, as in TS).
        let recording = node_semver_recording();
        let cases = recording["cases"]
            .as_array()
            .unwrap_or_else(|| unreachable!());
        assert!(cases.len() > 3_000, "{}", cases.len());
        let (mut accepted, mut lenient, mut beyond) = (0usize, 0usize, 0usize);
        for case in cases {
            let input = case["input"].as_str().unwrap_or_else(|| unreachable!());
            let actual = semver_parse(input).map(|v| {
                let prerelease: Vec<Value> = v
                    .prerelease
                    .iter()
                    .map(|id| match id {
                        PrereleaseIdentifier::Number(n) => crate::json!(n),
                        PrereleaseIdentifier::String(s) => crate::json!(s),
                    })
                    .collect();
                crate::json!({
                    "raw": v.raw,
                    "major": v.major,
                    "minor": v.minor,
                    "patch": v.patch,
                    "prerelease": prerelease,
                    "build": v.build,
                    "version": v.version,
                })
            });
            let expected = match &case["parsed"] {
                Value::Null => None,
                _ if ecma::js_trim(input) != input || input.starts_with('v') => {
                    // node-semver's trimming and `v` prefix: rejected (BC-41).
                    lenient += 1;
                    None
                }
                parsed => {
                    accepted += 1;
                    // JSON has one number type: compare the numbers as f64.
                    let mut parsed = parsed.clone();
                    for key in ["major", "minor", "patch"] {
                        parsed[key] = crate::json!(parsed[key].as_f64());
                    }
                    if let Some(ids) = parsed["prerelease"].as_array_mut() {
                        for id in ids.iter_mut() {
                            if let Some(n) = id.as_f64() {
                                *id = crate::json!(n);
                            }
                        }
                    }
                    Some(parsed)
                }
            };
            assert_eq!(actual, expected, "{input:?}");
            let beyond_limits = beyond_node_semver_limits(input);
            assert_eq!(
                is_strict_semver(input),
                (expected.is_some() && ecma::js_trim(input) == input && !input.starts_with('v'))
                    || beyond_limits,
                "{input:?}"
            );
            if beyond_limits {
                assert!(expected.is_none(), "{input:?}");
                beyond += 1;
            }
        }
        assert!(accepted > 1_000, "{accepted}");
        assert!(lenient > 10, "{lenient}");
        assert!(beyond > 0, "{beyond}");
    }

    #[test]
    fn id_regex_follows_the_ts_classes() {
        // Nd continues but does not start; Mn, Mc, Pc, ZWNJ and ZWJ continue;
        // a literal backslash-u escape is part of the name.
        assert!(is_valid_identifier("a\u{0663}"));
        assert!(!is_valid_identifier("\u{0663}a"));
        assert!(is_valid_identifier("a\u{0301}\u{200C}\u{200D}_"));
        assert!(is_valid_identifier(r"Abc"));
        assert!(!is_valid_identifier(""));
        assert!(!is_valid_identifier("with space"));
        // The *string* "undefined" is a valid identifier; a JS `undefined`
        // never reaches this function (BC-01).
        assert!(is_valid_identifier("undefined"));
    }

    #[test]
    fn semver_parse_is_strict_semver_2_0_0() {
        // BC-41: strict SemVer 2.0.0, no node-compat leniency.
        assert!(semver_parse("1.0.0").is_some());
        assert!(semver_parse("1.2.3-alpha.1+build.5").is_some());
        assert!(semver_parse(" v1.2.3-alpha.1+build.5 ").is_none());
        assert!(semver_parse("v1.0.0").is_none());
        assert!(semver_parse(" 1.0.0").is_none());
        assert!(semver_parse("1.0.0 ").is_none());
        assert!(semver_parse("1.1.2+.123").is_none());
        assert!(semver_parse("1.0").is_none());
        assert!(semver_parse("01.0.0").is_none());
        // Components go up to 2^64-1 (is_strict_semver), but beyond
        // node-semver's Number.MAX_SAFE_INTEGER there is no SemVer (TS's
        // `versionParsed` is null): see
        // `parse_namespace_beyond_node_semver_limits_has_no_version_parsed`.
        assert!(is_strict_semver("9007199254740992.0.0"));
        assert!(is_strict_semver(
            "18446744073709551615.18446744073709551615.18446744073709551615"
        ));
        assert!(!is_strict_semver("18446744073709551616.0.0"));
        assert!(!is_strict_semver("v1.0.0"));
        assert!(!is_strict_semver(" 1.0.0"));
        assert!(semver_parse("9007199254740992.0.0").is_none());
        assert!(semver_parse("18446744073709551616.0.0").is_none());
        let v = semver_parse("1.2.3-rc.10").unwrap_or_else(|| unreachable!());
        assert_eq!(v.version, "1.2.3-rc.10");
        assert_eq!(
            v.prerelease,
            vec![
                PrereleaseIdentifier::String("rc".into()),
                PrereleaseIdentifier::Number(10.0)
            ]
        );
        // No whitespace of any kind is trimmed.
        assert!(semver_parse("\u{0085}1.0.0").is_none());
        assert!(semver_parse("\u{FEFF}1.0.0").is_none());
    }

    /// The `versionParsed` of `org.acme@<version>`, which must be accepted.
    fn version_parsed_of(version: &str) -> Option<SemVer> {
        let ns = format!("org.acme@{version}");
        match parse_namespace_with(Some(&ns), false) {
            Ok(ParsedNamespace::Full {
                version: Some(v),
                version_parsed,
                ..
            }) => {
                assert_eq!(v, version);
                version_parsed
            }
            other => panic!("{ns}: {other:?}"),
        }
    }

    #[test]
    fn parse_namespace_beyond_node_semver_limits_has_no_version_parsed() {
        // Maintainer decision 2026-09-29: Rust matches TS. A strict version
        // beyond node-semver's limits is accepted (BC-41), but its
        // `versionParsed` is null, as `semver.parse` gives in TS.
        const SAFE: &str = "9007199254740991"; // 2^53-1
        const UNSAFE: &str = "9007199254740992"; // 2^53
        for (major, minor, patch) in [(SAFE, "0", "0"), ("0", SAFE, "0"), ("0", "0", SAFE)] {
            let v = version_parsed_of(&format!("{major}.{minor}.{patch}"))
                .unwrap_or_else(|| panic!("{major}.{minor}.{patch}"));
            assert_eq!(v.major + v.minor + v.patch, MAX_SAFE_INTEGER);
        }
        for version in [
            format!("{UNSAFE}.0.0"),
            format!("0.{UNSAFE}.0"),
            format!("0.0.{UNSAFE}"),
            "9007199254740993.0.0".to_string(),
            "18446744073709551615.18446744073709551615.18446744073709551615".to_string(),
        ] {
            assert_eq!(version_parsed_of(&version), None, "{version}");
        }
        // MAX_LENGTH: 256 UTF-16 units gives a SemVer, 257 none.
        let at_limit = format!("1.0.0-{}", "a".repeat(250));
        assert_eq!(at_limit.len(), 256);
        assert!(version_parsed_of(&at_limit).is_some());
        let over = format!("1.0.0-{}", "a".repeat(251));
        assert_eq!(version_parsed_of(&over), None);
        // Still rejected: not strict SemVer 2.0.0, or above 2^64-1.
        for ns in ["org.acme@v1.0.0", "org.acme@18446744073709551616.0.0"] {
            assert!(parse_namespace_with(Some(ns), false).is_err(), "{ns}");
        }
    }

    #[test]
    fn capitalize_first_letter_counts_utf16_units() {
        assert_eq!(capitalize_first_letter(""), "");
        assert_eq!(capitalize_first_letter("ßa"), "SSa");
        // U+10428 DESERET SMALL LETTER LONG I has an upper case, but JS
        // upper-cases only the lone high surrogate.
        assert_eq!(capitalize_first_letter("\u{10428}a"), "\u{10428}a");
    }

    #[test]
    fn import_fully_qualified_names_covers_every_import_kind() {
        let names =
            |imp: Value| import_fully_qualified_names(Some(&imp)).map_err(|e| e.to_string());
        assert_eq!(
            names(
                crate::json!({"$class": "concerto.metamodel@1.0.0.ImportAll", "namespace": "a@1.0.0"})
            ),
            Ok(vec!["a@1.0.0.*".to_string()])
        );
        assert_eq!(
            names(
                crate::json!({"$class": "concerto.metamodel@1.0.0.ImportType", "namespace": "a@1.0.0", "name": "B"})
            ),
            Ok(vec!["a@1.0.0.B".to_string()])
        );
        assert_eq!(
            names(
                crate::json!({"$class": "concerto.metamodel@1.0.0.ImportTypes", "namespace": "a@1.0.0", "types": ["B", "C"]})
            ),
            Ok(vec!["a@1.0.0.B".to_string(), "a@1.0.0.C".to_string()])
        );
        assert_eq!(
            names(
                crate::json!({"$class": "concerto.metamodel@1.0.0.ImportTypes", "namespace": "a@1.0.0"})
            ),
            Err("Cannot read properties of undefined (reading 'forEach')".to_string())
        );
        assert_eq!(
            names(crate::json!({"$class": "ImportAll"})),
            Err("Unrecognized imports ImportAll".to_string())
        );
    }

    /// One test function per `it()` in `test/modelutil.js`,
    /// named after its `describe`/`it` titles, so `grep` finds the port of
    /// each assertion (PORTING.md 10.11). The 6 `#isAssignableTo` cases are
    /// tagged `W` (they stub `ModelFile`/`Property`/`ModelManager` with
    /// sinon): `ModelUtil.isAssignableTo` is exercised instead against a real
    /// arena-backed `ModelManager` in `model_manager::tests::
    /// ported_members_run_on_the_arena` and `model_manager::tests`'
    /// `is_assignable_to` cases.
    mod ts_modelutil_js {
        use super::*;

        // #isPrimitiveType > check isPrimitiveType
        #[test]
        fn is_primitive_type_check_is_primitive_type() {
            assert!(!is_primitive_type("org.acme.baz@1.0.0.Foo"));
            assert!(is_primitive_type("Boolean"));
            assert!(is_primitive_type("Integer"));
            assert!(is_primitive_type("Long"));
            assert!(is_primitive_type("DateTime"));
            assert!(is_primitive_type("String"));
        }

        // #getShortName > should handle a name with a namespace
        #[test]
        fn get_short_name_should_handle_a_name_with_a_namespace() {
            assert_eq!(short_name("org.acme.baz@1.0.0.Foo"), "Foo");
        }

        // #getShortName > should handle a name without a namespace
        #[test]
        fn get_short_name_should_handle_a_name_without_a_namespace() {
            assert_eq!(short_name("Foo"), "Foo");
        }

        // #getNamespace > check getNamespace
        #[test]
        fn get_namespace_check_get_namespace() {
            assert_eq!(
                get_namespace(Some("org.acme.baz@1.0.0.Foo")).unwrap(),
                "org.acme.baz@1.0.0"
            );
            assert_eq!(get_namespace(Some("Foo")).unwrap(), "");
        }

        // #capitalizeFirstLetter > should handle a single lower case letter
        #[test]
        fn capitalize_first_letter_should_handle_a_single_lower_case_letter() {
            assert_eq!(capitalize_first_letter("a"), "A");
        }

        // #capitalizeFirstLetter > should handle a single upper case letter
        #[test]
        fn capitalize_first_letter_should_handle_a_single_upper_case_letter() {
            assert_eq!(capitalize_first_letter("A"), "A");
        }

        // #capitalizeFirstLetter > should handle a string of lower case letters
        #[test]
        fn capitalize_first_letter_should_handle_a_string_of_lower_case_letters() {
            assert_eq!(capitalize_first_letter("abcdef"), "Abcdef");
        }

        // #capitalizeFirstLetter > should handle a string of mixed case letters
        #[test]
        fn capitalize_first_letter_should_handle_a_string_of_mixed_case_letters() {
            assert_eq!(capitalize_first_letter("aBcDeF"), "ABcDeF");
        }

        // #getFullyQualifiedName > valid inputs
        #[test]
        fn get_fully_qualified_name_valid_inputs() {
            assert_eq!(qualify("a.namespace", "type"), "a.namespace.type");
        }

        // #getFullyQualifiedName > empty namespace should return the type with no leading dot
        #[test]
        fn get_fully_qualified_name_empty_namespace_should_return_the_type_with_no_leading_dot() {
            assert_eq!(qualify("", "type"), "type");
        }

        // #removeNamespaceVersionFromFullyQualifiedName > valid inputs
        #[test]
        fn remove_namespace_version_from_fully_qualified_name_valid_inputs() {
            assert_eq!(
                remove_namespace_version_from_fully_qualified_name(Some("org.acme@1.0.0.Person"))
                    .unwrap(),
                "org.acme.Person"
            );
        }

        // #removeNamespaceVersionFromFullyQualifiedName > primtive type [sic]
        #[test]
        fn remove_namespace_version_from_fully_qualified_name_primitive_type() {
            assert_eq!(
                remove_namespace_version_from_fully_qualified_name(Some("String")).unwrap(),
                "String"
            );
        }

        // #parseNamespace > valid, with version
        #[test]
        fn parse_namespace_valid_with_version() {
            let ParsedNamespace::Full {
                name,
                escaped_namespace,
                version,
                version_parsed,
            } = parse_namespace_with(Some("org.acme@1.0.0"), false).unwrap()
            else {
                unreachable!("version parsing is not disabled")
            };
            assert_eq!(name, "org.acme");
            assert_eq!(escaped_namespace, "org.acme_1.0.0");
            assert_eq!(version.as_deref(), Some("1.0.0"));
            assert_eq!(version_parsed.unwrap().major, 1.0);
        }

        // #parseNamespace > valid, with version validation disabled
        #[test]
        fn parse_namespace_valid_with_version_validation_disabled() {
            // TS calls `parseNamespace('org.acme@1.0.x', {
            // disableVersionParsing: true })`; `1.0.x` is never validated as a
            // semver, and the result carries `name` only (no
            // `escapedNamespace`/`version`/`versionParsed` properties).
            let ParsedNamespace::NameOnly { name } =
                parse_namespace_with(Some("org.acme@1.0.x"), true).unwrap()
            else {
                unreachable!("version parsing is disabled")
            };
            assert_eq!(name, "org.acme");
        }

        // #parseNamespace > invalid (null)
        #[test]
        fn parse_namespace_invalid_null() {
            let err = parse_namespace_with(None, false).unwrap_err();
            assert!(err.to_string().contains("Namespace is null"), "{err}");
        }

        // #parseNamespace > invalid (org.acme@1.0.0@2.3)
        #[test]
        fn parse_namespace_invalid_two_at_signs() {
            let err = parse_namespace_with(Some("org.acme@1.0.0@2.3"), false).unwrap_err();
            assert!(err.to_string().contains("Invalid namespace"), "{err}");
        }

        // #parseNamespace > invalid version
        #[test]
        fn parse_namespace_invalid_version() {
            let err = parse_namespace_with(Some("org.acme@1.1.2+.123"), false).unwrap_err();
            assert!(err.to_string().contains("Invalid namespace"), "{err}");
        }

        // BC-02: an unversioned namespace is rejected,
        // with the error an invalid one gets, whether or not the version
        // is parsed.
        #[test]
        fn parse_namespace_rejects_an_unversioned_namespace() {
            for disable in [false, true] {
                for ns in ["org.acme", "concerto", "a"] {
                    let err = parse_namespace_with(Some(ns), disable).unwrap_err();
                    assert_eq!(err.contract().kind, ErrorKind::InvalidArgument, "{ns}");
                    assert!(err.to_string().contains("Invalid namespace"), "{ns}: {err}");
                }
            }
            assert!(parse_namespace("org.acme").is_err());
        }
    }
}
