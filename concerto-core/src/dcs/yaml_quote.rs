//! A Rust port of `DecoratorExtractor.quoteStringValue`
//! (`src/decoratorextractor.ts`), and of the small slice of the `yaml` npm
//! library (`yaml.stringify`, v2.9.0, vendored under `node_modules/yaml` in
//! the `concerto` checkout at `claude/tender-pascal-ocwf9q`) that its
//! quoting decision depends on.
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
//! `yaml.stringify(str)` renders `str` as the sole document value: not a
//! mapping key (`implicitKey: false`) and not inside a flow collection
//! (`inFlow: false`), through `stringifyString`/`plainString`
//! (`dist/stringify/stringifyString.js`). Rather than re-derive the decision
//! from the YAML 1.2 spec, this walks that source directly so the port
//! tracks the exact rules the reference embeds (character classes, the core
//! schema's type-collision tags, and `foldFlowLines`' line width folding at
//! the library's default `lineWidth: 80`), including its two eemeli/yaml-only
//! quirks: an empty string counts as a `null` scalar (`nullTag`'s test is
//! `/^(?:~|[Nn]ull|NULL)?$/`, whose group is optional), and the returned
//! quoted form is `JSON.stringify`, not YAML's own double-quote escaping.
//!
//! Every branch of `plainString` other than its final "no folding needed"
//! return produces a rendering that can never equal `str` after
//! `trimEnd()`: a quoted or block scalar always opens with a quote, `|`, `>`,
//! `%`/`---`/`...` marker, or (for a control character) a forced double
//! quote, none of which `str` itself starts with. [`needs_quoting`] uses
//! that to collapse the reference's branches into a single yes/no decision
//! instead of reconstructing every rendering.
use std::sync::LazyLock;

use regress::Regex;

/// `DECORATOR_STRING_TYPE` (`src/decoratorextractor.ts`): the `$class` of a
/// decorator argument that carries a string value.
pub const DECORATOR_STRING_TYPE: &str = "concerto.metamodel@1.0.0.DecoratorString";

/// `plainString`'s "not allowed" test (`dist/stringify/stringifyString.js`):
/// starts with an indicator character (other than `?`/`-`), is exactly `?`
/// or `-`, starts with `? `/`- ` (or a tab), has `\n `/`: ` (or a tab)
/// anywhere, has ` \n` (or a tab) anywhere, has `\n#`/` #`/`\t#` anywhere, or
/// ends in whitespace or `:`.
const FORBIDDEN_PATTERN: &str =
    r#"^[\n\t ,\[\]{}#&*!|>'"%@`]|^[?-]$|^[?-][ \t]|[\n:][ \t]|[ \t]\n|[\n\t ]#|[\n\t :]$"#;

/// `containsDocumentMarker` (`dist/stringify/stringifyString.js`): a line
/// that could be mistaken for a YAML 1.1 document marker. Only the `^`
/// anchor without the `m` flag is needed here: any value reaching this check
/// has already been confirmed not to contain a newline (see
/// [`needs_quoting`]'s doc comment).
const DOCUMENT_MARKER_PATTERN: &str = r"^(%|---|\.\.\.)";

/// The `core` schema's tags with `default: true` and a `tag` other than
/// `tag:yaml.org,2002:str` (`dist/schema/core/schema.js`,
/// `dist/schema/common/null.js`), in the order `plainString`'s
/// `tags.some(test)` would try them. `yaml.stringify` with no `options`
/// resolves the default (`core`) schema, so these are the tags a plain
/// scalar's rendering is checked against to avoid it being read back as a
/// non-string type.
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

/// `stringifyString`'s control-character check
/// (`dist/stringify/stringifyString.js`): forces double-quote style. The `u`
/// flag's unpaired-surrogate class (`\u{D800}-\u{DFFF}`) has no Rust
/// counterpart — a `&str` is always valid UTF-8 and can hold no lone
/// surrogate (DV-004's WASM-boundary analysis applies the same way here).
fn has_control_character(value: &str) -> bool {
    value
        .chars()
        .any(|c| matches!(c as u32, 0x00..=0x08 | 0x0b..=0x1f | 0x7f..=0x9f))
}

/// `foldFlowLines(text, "", "flow", { lineWidth: 80, minContentWidth: 20 })`
/// (`dist/stringify/foldFlowLines.js`), reduced to the yes/no this module
/// needs: does folding a single-line, already-plain-safe `value` at the
/// library's default `lineWidth` change it at all? `indent` is always `""`
/// here (`quoteStringValue` renders at the document root), which drops the
/// per-fold `consumeMoreIndentedLines`/block-mode branches (`mode ===
/// FOLD_BLOCK`) and the `indentAtStart` branch (never set for a root
/// scalar): a fold only ever replaces one of `value`'s spaces with `"\n"`,
/// which cannot equal `value` again. So folding is exactly "does
/// `foldFlowLines` find at least one fold", which is what this returns.
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

/// Does `value` need to be wrapped in double quotes to appear in extracted
/// vocabulary YAML, per `yaml.stringify(value).trimEnd() === value`
/// (`DecoratorExtractor.quoteStringValue`)? See the module doc comment for
/// why every branch but the reference's final "plain, unfolded" one can be
/// collapsed into `true` here.
fn needs_quoting(value: &str) -> bool {
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
    if CORE_SCHEMA_TYPES.iter().any(|re| re.find(value).is_some()) {
        return true;
    }
    would_fold(value)
}

/// `JSON.stringify(str)` (`dist/stringify/stringifyString.js`'s
/// `doubleQuotedString` is not what `quoteStringValue` actually calls for
/// its quoted branch — see the module doc comment). Escapes `"`, `\`, and
/// the C0 control characters the JSON grammar requires (`\b \f \n \r \t`,
/// `\u00XX` otherwise); leaves every other Unicode scalar value, including
/// non-ASCII text, as-is, exactly as `serde_json`'s `Display` for
/// `Value::String` does.
fn json_quote(value: &str) -> String {
    serde_json::to_string(value).expect("a &str always serialises")
}

/// A value safe for embedding in a YAML scalar. String values containing
/// YAML-special characters are wrapped in double quotes; non-string
/// decorator argument types (Number, Boolean) are returned unchanged.
///
/// TS: `DecoratorExtractor.quoteStringValue` (`src/decoratorextractor.ts`).
/// `value` is `String(value)` already applied — see
/// [`crate::dcs::extractor::DecoratorExtractor`]'s call sites, which apply
/// the same JS `String()` coercion the reference does before reaching here.
pub fn quote_string_value(value: &str, type_class: Option<&str>) -> String {
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
mod tests {
    use super::*;

    fn quote(value: &str) -> String {
        quote_string_value(value, Some(DECORATOR_STRING_TYPE))
    }

    #[test]
    fn non_string_argument_types_pass_through_unchanged() {
        assert_eq!(
            quote_string_value("42", Some("concerto.metamodel@1.0.0.DecoratorNumber")),
            "42"
        );
        assert_eq!(quote_string_value("42", None), "42");
    }

    #[test]
    fn a_plain_word_is_left_unquoted() {
        assert_eq!(quote("hello"), "hello");
        assert_eq!(quote("Hello, World"), "Hello, World");
        assert_eq!(quote("a-simple-term"), "a-simple-term");
    }

    #[test]
    fn an_empty_string_is_quoted_as_it_would_parse_back_as_null() {
        assert_eq!(quote(""), "\"\"");
    }

    #[test]
    fn strings_that_look_like_other_scalar_types_are_quoted() {
        for s in [
            "true", "True", "TRUE", "false", "null", "Null", "NULL", "~", "42", "-7", "+3", "0x1F",
            "0o17", "3.14", "-0.5", "1e10", "1.5e-3", ".inf", "-.inf", ".nan",
        ] {
            assert_eq!(quote(s), json_quote(s), "expected {s:?} to be quoted");
        }
    }

    #[test]
    fn a_string_that_is_not_a_number_stays_plain() {
        // Not a full match for the int/float tags (trailing non-digit).
        assert_eq!(quote("42a"), "42a");
        assert_eq!(quote("v1.2.3"), "v1.2.3");
    }

    #[test]
    fn leading_indicator_characters_are_quoted() {
        for s in [
            "- item", "-", "?", "? key", "#comment", "*anchor", "&anchor", "!tag", "|lit", ">fold",
            "'q'", "\"q\"", "%tag", "@at", "`tick`", ",comma", "[seq", "{map",
        ] {
            assert_eq!(quote(s), json_quote(s), "expected {s:?} to be quoted");
        }
    }

    #[test]
    fn a_colon_space_anywhere_forces_quoting() {
        assert_eq!(quote("key: value"), json_quote("key: value"));
        assert_eq!(quote("a:b"), "a:b");
    }

    #[test]
    fn trailing_whitespace_or_colon_forces_quoting() {
        assert_eq!(quote("trailing "), json_quote("trailing "));
        assert_eq!(quote("trailing:"), json_quote("trailing:"));
    }

    #[test]
    fn an_embedded_hash_preceded_by_space_forces_quoting() {
        assert_eq!(quote("a #comment"), json_quote("a #comment"));
        // Not preceded by whitespace: allowed plain.
        assert_eq!(quote("a#b"), "a#b");
    }

    #[test]
    fn a_newline_always_forces_quoting() {
        assert_eq!(
            quote("line one\nline two"),
            json_quote("line one\nline two")
        );
    }

    #[test]
    fn a_document_marker_line_forces_quoting() {
        assert_eq!(quote("---"), json_quote("---"));
        assert_eq!(
            quote("--- not a marker really"),
            json_quote("--- not a marker really")
        );
        assert_eq!(quote("..."), json_quote("..."));
        assert_eq!(quote("%YAML 1.2"), json_quote("%YAML 1.2"));
    }

    #[test]
    fn control_characters_force_quoting() {
        assert_eq!(quote("a\tb\u{0007}c"), json_quote("a\tb\u{0007}c"));
    }

    #[test]
    fn non_ascii_text_is_left_plain() {
        assert_eq!(quote("café"), "café");
        assert_eq!(quote("日本語"), "日本語");
    }

    #[test]
    fn a_short_string_under_the_line_width_is_never_folded() {
        let s = "a ".repeat(30) + "z"; // 61 chars, well under 80
        assert_eq!(quote(&s), s);
    }

    #[test]
    fn a_long_string_past_the_line_width_is_folded_and_so_quoted() {
        let s = "word ".repeat(30); // 150 chars, folds past width 80
        assert_eq!(quote(&s), json_quote(&s));
    }

    #[test]
    fn a_long_string_with_no_fold_point_is_left_plain() {
        // One giant run with no space after column 80 to split on.
        let s = "x".repeat(200);
        assert_eq!(quote(&s), s);
    }
}
