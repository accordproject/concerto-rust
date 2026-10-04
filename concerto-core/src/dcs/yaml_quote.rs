//! A port of `DecoratorExtractor.quoteStringValue`
//! (`src/decoratorextractor.ts`), and of the slice of the `yaml` npm library
//! (`yaml.stringify`, v2.9.0) its quoting decision depends on.
//!
//! `quoteStringValue` decides whether a decorator argument's string value can
//! be written into extracted vocabulary YAML unquoted (a YAML "plain
//! scalar") or needs to be wrapped in JSON-style double quotes:
//!
//! ```js
//! quoteStringValue(value, type) {
//!     if (type !== DECORATOR_STRING_TYPE) return value;
//!     const str = String(value);
//!     const serialized = yaml.stringify(str).trimEnd();
//!     return serialized === str ? str : JSON.stringify(str);
//! }
//! ```
//!
//! `yaml.stringify(str)` renders `str` as the sole document value (not a
//! mapping key, not in a flow collection) through `stringifyString`/
//! `plainString` (`dist/stringify/stringifyString.js`). This follows that
//! source rather than the YAML 1.2 spec: its character classes, the core
//! schema's type-collision tags, `foldFlowLines` at the default
//! `lineWidth: 80`, and two quirks: an empty string counts as `null`
//! (`nullTag`'s group is optional), and the quoted form is `JSON.stringify`.
//!
//! Every branch of `plainString` but its final "no folding needed" return
//! renders something that opens with a quote, `|`, `>` or a document marker,
//! so it never equals `str` after `trimEnd()`. [`needs_quoting`] therefore
//! collapses those branches into one yes/no decision.
use std::sync::LazyLock;

use regress::Regex;

/// `DECORATOR_STRING_TYPE` (`src/decoratorextractor.ts`): the `$class` of a
/// decorator argument that carries a string value.
pub(crate) const DECORATOR_STRING_TYPE: &str = "concerto.metamodel@1.0.0.DecoratorString";

/// `plainString`'s "not allowed" test (`dist/stringify/stringifyString.js`):
/// starts with an indicator character (other than `?`/`-`), is exactly `?`
/// or `-`, starts with `? `/`- ` (or a tab), has `\n `/`: ` (or a tab)
/// anywhere, has ` \n` (or a tab) anywhere, has `\n#`/` #`/`\t#` anywhere, or
/// ends in whitespace or `:`.
const FORBIDDEN_PATTERN: &str =
    r#"^[\n\t ,\[\]{}#&*!|>'"%@`]|^[?-]$|^[?-][ \t]|[\n:][ \t]|[ \t]\n|[\n\t ]#|[\n\t :]$"#;

/// `containsDocumentMarker` (`dist/stringify/stringifyString.js`). No `m`
/// flag is needed: a value reaching this check has no newline.
const DOCUMENT_MARKER_PATTERN: &str = r"^(%|---|\.\.\.)";

/// The default (`core`) schema's tags with `default: true` and a non-`str`
/// tag (`dist/schema/core/schema.js`, `dist/schema/common/null.js`), in the
/// order `plainString`'s `tags.some(test)` tries them: a plain scalar
/// matching one would read back as a non-string.
const CORE_SCHEMA_TYPE_PATTERNS: &[&str] = &[
    // nullTag (dist/schema/common/null.js) — the group is optional, so this
    // also matches the empty string.
    r"^(?:~|[Nn]ull|NULL)?$",
    // boolTag (dist/schema/core/bool.js)
    r"^(?:[Tt]rue|TRUE|[Ff]alse|FALSE)$",
    // intOct (dist/schema/core/int.js)
    r"^0o[0-7]+$",
    // int (dist/schema/core/int.js)
    r"^[-+]?[0-9]+$",
    // intHex (dist/schema/core/int.js)
    r"^0x[0-9a-fA-F]+$",
    // floatNaN (dist/schema/core/float.js)
    r"^(?:[-+]?\.(?:inf|Inf|INF)|\.nan|\.NaN|\.NAN)$",
    // floatExp (dist/schema/core/float.js)
    r"^[-+]?(?:\.[0-9]+|[0-9]+(?:\.[0-9]*)?)[eE][-+]?[0-9]+$",
    // float (dist/schema/core/float.js)
    r"^[-+]?(?:\.[0-9]+|[0-9]+\.[0-9]*)$",
];

fn compile(pattern: &str) -> Regex {
    Regex::new(pattern).unwrap_or_else(|e| panic!("invalid pattern {pattern:?}: {e}"))
}

static FORBIDDEN: LazyLock<Regex> = LazyLock::new(|| compile(FORBIDDEN_PATTERN));
static DOCUMENT_MARKER: LazyLock<Regex> = LazyLock::new(|| compile(DOCUMENT_MARKER_PATTERN));
static CORE_SCHEMA_TYPES: LazyLock<Vec<Regex>> = LazyLock::new(|| {
    CORE_SCHEMA_TYPE_PATTERNS
        .iter()
        .map(|p| compile(p))
        .collect()
});

/// `stringifyString`'s control-character check, which forces double quotes.
/// The `u` flag's lone-surrogate class has no counterpart: a `&str` cannot
/// hold a lone surrogate (DV-004).
fn has_control_character(value: &str) -> bool {
    value
        .chars()
        .any(|c| matches!(c as u32, 0x00..=0x08 | 0x0b..=0x1f | 0x7f..=0x9f))
}

/// `foldFlowLines(text, "", "flow", { lineWidth: 80, minContentWidth: 20 })`
/// (`dist/stringify/foldFlowLines.js`), reduced to: does folding a
/// single-line, plain-safe `value` at the root change it? A fold replaces
/// one of `value`'s spaces with `"\n"`, so this is whether `foldFlowLines`
/// finds at least one fold.
fn would_fold(value: &str) -> bool {
    const LINE_WIDTH: usize = 80;
    const MIN_CONTENT_WIDTH: usize = 20;
    let end_step = (1 + MIN_CONTENT_WIDTH).max(1 + LINE_WIDTH);
    let chars: Vec<char> = value.chars().collect();
    if chars.len() <= end_step {
        return false;
    }
    let end = LINE_WIDTH;
    let mut split: Option<usize> = None;
    let mut prev: Option<char> = None;
    for (i, &ch) in chars.iter().enumerate() {
        if ch == ' ' && matches!(prev, Some(p) if p != ' ' && p != '\n' && p != '\t') {
            let next = chars.get(i + 1).copied();
            if matches!(next, Some(n) if n != ' ' && n != '\n' && n != '\t') {
                split = Some(i);
            }
        }
        if i >= end && split.is_some() {
            // A fold point past the line width: `foldFlowLines` records it
            // and keeps scanning for more, but one is already enough to know
            // the rendering changes (see the doc comment above).
            return true;
        }
        // FOLD_FLOW (not FOLD_QUOTED): an overflow with no fold point found
        // yet just keeps scanning (`overflow = true`), it does not itself
        // fold.
        prev = Some(ch);
    }
    false
}

/// Whether `value` needs double quotes in extracted vocabulary YAML, per
/// `yaml.stringify(value).trimEnd() === value` under the default `core`
/// schema (module doc).
fn needs_quoting(value: &str) -> bool {
    needs_quoting_with(value, true)
}

/// The same decision as [`needs_quoting`], for the `failsafe` schema
/// (`dcsconverter.ts`'s `yaml.stringify(_, {schema: 'failsafe'})`), which
/// has no null/bool/int/float tags to collide with: `"true"`, `"2.5"` and
/// `"null"` stay plain.
pub(crate) fn needs_quoting_failsafe(value: &str) -> bool {
    needs_quoting_with(value, false)
}

fn needs_quoting_with(value: &str, check_core_schema_types: bool) -> bool {
    if has_control_character(value) {
        return true;
    }
    if FORBIDDEN.find(value).is_some() {
        return true;
    }
    // `plainString`'s multiline branch (`!implicitKey && !inFlow && type !==
    // Scalar.PLAIN && value.includes('\n')`) always applies here: the
    // synthetic scalar `yaml.stringify` builds for a raw JS string never
    // carries an explicit `type`, so it is never `Scalar.PLAIN` either.
    // `blockString` always opens with `|`/`>`, so this is always a mismatch.
    if value.contains('\n') {
        return true;
    }
    if DOCUMENT_MARKER.find(value).is_some() {
        return true;
    }
    if check_core_schema_types && CORE_SCHEMA_TYPES.iter().any(|re| re.find(value).is_some()) {
        return true;
    }
    would_fold(value)
}

/// `JSON.stringify(str)`, the quoted form `quoteStringValue` returns (module
/// doc), as `serde_json`'s `Display` for `Value::String` writes it. Also the
/// double-quoted rendering [`dcsconverter`](crate::dcs::dcsconverter) picks
/// when a `failsafe` plain scalar is not safe ([`needs_quoting_failsafe`]).
pub(crate) fn json_quote(value: &str) -> String {
    serde_json::to_string(value).expect("a &str always serialises")
}

/// A value safe for embedding in a YAML scalar: a string with YAML-special
/// characters is double-quoted; other argument types are unchanged.
///
/// TS: `DecoratorExtractor.quoteStringValue` (`src/decoratorextractor.ts`).
/// `value` already has JS `String()` applied by the caller.
pub(crate) fn quote_string_value(value: &str, type_class: Option<&str>) -> String {
    if type_class != Some(DECORATOR_STRING_TYPE) {
        return value.to_string();
    }
    if needs_quoting(value) {
        json_quote(value)
    } else {
        value.to_string()
    }
}

#[cfg(test)]
#[path = "tests/yaml_quote.rs"]
mod tests;
