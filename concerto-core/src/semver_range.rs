//! A port of node-semver 7.6.3's range grammar and `satisfies`, for
//! [`crate::introspect::model_file::ModelFile`]'s `concertoVersion` check
//! (`isCompatibleVersion` in `modelfile.js`; P2-08 review). TS calls
//! `semver.satisfies(packageJson.version, this.ast.concertoVersion,
//! {includePrerelease: true})`, then, on failure,
//! `semver.minSatisfying(['3.0.0'], this.ast.concertoVersion)` (no options)
//! for the v3 backward-compatibility fallback; both go through
//! [`satisfies`], which is what this module ports.
//!
//! The Cargo `semver` crate this replaced covers Cargo's own requirement
//! syntax, not node-semver's: it rejects space-separated AND comparators
//! (`>=3.0.0 <6.0.0`) and hyphen ranges (`1.2.3 - 2.3.4`) outright, and reads
//! a bare version (`5.1.0`) as `^5.1.0` rather than node-semver's exact
//! `=5.1.0`. A model using any of those common node-semver idioms was
//! rejected by the Rust engine and accepted by TS.
//!
//! This follows node-semver's own pipeline (`classes/range.js`,
//! `internal/parse-options.js`): collapse whitespace, split on `||` into
//! range-sets, expand each range-set's hyphen range or `^`/`~`/X-range/plain
//! comparator tokens into concrete `<op> <version>` comparators AND'd
//! together, then test the AND of one range-set, OR'd across all of them.
//!
//! **One deliberate simplification.** node-semver appends a `-0` (lowest
//! possible prerelease) marker to an exclusive upper bound it derives from a
//! caret, tilde, X-range or hyphen-range shorthand (`^1.2.3` becomes
//! `>=1.2.3 <2.0.0-0`, not `<2.0.0`), so that a prerelease of the *next*
//! version is still excluded even though `includePrerelease` is set. This
//! only changes the answer for a candidate version that is itself a
//! prerelease of that next version; both calls this module serves test a
//! plain release (`packageJson.version` and the literal `'3.0.0'`), which a
//! `-0` marker never affects (a release always compares above every
//! prerelease of the same `[major, minor, patch]`, marker or not). This
//! module builds plain integer boundaries and omits the marker; it still
//! compares prerelease identifiers per the semver spec wherever a range's
//! own comparator carries one explicitly (`^1.2.3-beta.1`), since that is
//! reachable and does affect the general-purpose functions here.

use std::cmp::Ordering;
use std::sync::LazyLock;

use crate::model_util::{PrereleaseIdentifier, SemVer, semver_parse};

/// A partial version's major/minor/patch component: a concrete number, or a
/// wildcard (`x`, `X`, `*`, or simply omitted — node-semver's X-Ranges treat
/// a missing component the same as an explicit wildcard one).
#[derive(Clone, Copy, Debug)]
enum Part {
    Num(f64),
    Wild,
}

impl Part {
    fn num(self) -> Option<f64> {
        match self {
            Self::Num(n) => Some(n),
            Self::Wild => None,
        }
    }
}

/// A version as one comparator token names it, before `^`/`~`/X-range
/// expansion: each component may be a wildcard, and only the patch
/// component may carry a prerelease (node-semver's `XRANGEPLAIN`/tilde/caret
/// tokens all nest prerelease under a concrete patch).
struct Partial {
    major: Part,
    minor: Part,
    patch: Part,
    prerelease: Vec<PrereleaseIdentifier>,
}

/// node-semver's partial-version grammar (`XRANGEPLAIN`/`TILDE`/`CARET`
/// tokens all share this shape): an optional leading `v`, then
/// major[.minor[.patch[-prerelease][+build]]], any of the three numeric
/// components standing in for `x`/`X`/`*`.
const PARTIAL_VERSION_PATTERN: &str = r"^v?(0|[1-9]\d*|[xX]|\*)(?:\.(0|[1-9]\d*|[xX]|\*)(?:\.(0|[1-9]\d*|[xX]|\*)(?:-((?:0|[1-9]\d*|\d*[a-zA-Z-][a-zA-Z0-9-]*)(?:\.(?:0|[1-9]\d*|\d*[a-zA-Z-][a-zA-Z0-9-]*))*))?(?:\+([0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*))?)?)?$";

static PARTIAL_VERSION_REGEX: LazyLock<regress::Regex> = LazyLock::new(|| {
    // A constant pattern; `partial_version_pattern_compiles` tests that it
    // compiles.
    regress::Regex::new(PARTIAL_VERSION_PATTERN).expect("PARTIAL_VERSION_PATTERN is valid")
});

fn parse_partial(s: &str) -> Option<Partial> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }
    let m = PARTIAL_VERSION_REGEX.find(s)?;
    let group = |i: usize| m.group(i).map(|range| &s[range]);
    let part = |i: usize| match group(i) {
        None => Part::Wild,
        Some("x" | "X" | "*") => Part::Wild,
        Some(digits) => digits.parse::<f64>().map_or(Part::Wild, Part::Num),
    };
    let prerelease = match group(4) {
        None | Some("") => Vec::new(),
        Some(ids) => ids
            .split('.')
            .map(|id| {
                if !id.is_empty()
                    && id.bytes().all(|b| b.is_ascii_digit())
                    && let Ok(n) = id.parse::<f64>()
                {
                    return PrereleaseIdentifier::Number(n);
                }
                PrereleaseIdentifier::String(id.to_string())
            })
            .collect(),
        // group(5) (build metadata) plays no part in ordering or bounds.
    };
    Some(Partial {
        major: part(1),
        minor: part(2),
        patch: part(3),
        prerelease,
    })
}

fn concrete(major: f64, minor: f64, patch: f64, prerelease: &[PrereleaseIdentifier]) -> SemVer {
    SemVer {
        raw: String::new(),
        major,
        minor,
        patch,
        prerelease: prerelease.to_vec(),
        build: Vec::new(),
        version: String::new(),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Op {
    Eq,
    Gt,
    Gte,
    Lt,
    Lte,
}

/// One expanded comparator (node-semver's `Comparator`): `Any` is an
/// unconstrained bound (`*`, or a wildcard major with a non-strict
/// operator); `Never` is `<0.0.0` (a wildcard major with `>` or `<`, which
/// nothing satisfies); `Bound` is a concrete `<op> <version>`.
enum Comp {
    Any,
    Never,
    Bound { op: Op, version: SemVer },
}

/// node-semver `replaceCaret`.
fn caret_bounds(p: &Partial) -> Vec<Comp> {
    let Some(major) = p.major.num() else {
        return vec![Comp::Any];
    };
    let Some(minor) = p.minor.num() else {
        return vec![
            Comp::Bound {
                op: Op::Gte,
                version: concrete(major, 0.0, 0.0, &[]),
            },
            Comp::Bound {
                op: Op::Lt,
                version: concrete(major + 1.0, 0.0, 0.0, &[]),
            },
        ];
    };
    let Some(patch) = p.patch.num() else {
        return if major == 0.0 {
            vec![
                Comp::Bound {
                    op: Op::Gte,
                    version: concrete(major, minor, 0.0, &[]),
                },
                Comp::Bound {
                    op: Op::Lt,
                    version: concrete(major, minor + 1.0, 0.0, &[]),
                },
            ]
        } else {
            vec![
                Comp::Bound {
                    op: Op::Gte,
                    version: concrete(major, minor, 0.0, &[]),
                },
                Comp::Bound {
                    op: Op::Lt,
                    version: concrete(major + 1.0, 0.0, 0.0, &[]),
                },
            ]
        };
    };
    let lower = concrete(major, minor, patch, &p.prerelease);
    let upper = if major == 0.0 {
        if minor == 0.0 {
            concrete(major, minor, patch + 1.0, &[])
        } else {
            concrete(major, minor + 1.0, 0.0, &[])
        }
    } else {
        concrete(major + 1.0, 0.0, 0.0, &[])
    };
    vec![
        Comp::Bound {
            op: Op::Gte,
            version: lower,
        },
        Comp::Bound {
            op: Op::Lt,
            version: upper,
        },
    ]
}

/// node-semver `replaceTilde`.
fn tilde_bounds(p: &Partial) -> Vec<Comp> {
    let Some(major) = p.major.num() else {
        return vec![Comp::Any];
    };
    let Some(minor) = p.minor.num() else {
        return vec![
            Comp::Bound {
                op: Op::Gte,
                version: concrete(major, 0.0, 0.0, &[]),
            },
            Comp::Bound {
                op: Op::Lt,
                version: concrete(major + 1.0, 0.0, 0.0, &[]),
            },
        ];
    };
    let (patch, prerelease): (f64, &[PrereleaseIdentifier]) = match p.patch.num() {
        Some(patch) => (patch, &p.prerelease),
        None => (0.0, &[]),
    };
    vec![
        Comp::Bound {
            op: Op::Gte,
            version: concrete(major, minor, patch, prerelease),
        },
        Comp::Bound {
            op: Op::Lt,
            version: concrete(major, minor + 1.0, 0.0, &[]),
        },
    ]
}

/// node-semver `replaceXRange`, for a plain comparator: `op` is the
/// operator a token was given (`None` for a bare version, node-semver's
/// `gtlt === ''`, which an explicit `=` on a wildcard version also becomes).
fn xrange_bounds(op: Option<Op>, p: &Partial) -> Vec<Comp> {
    let Some(major) = p.major.num() else {
        return match op {
            Some(Op::Gt | Op::Lt) => vec![Comp::Never],
            _ => vec![Comp::Any],
        };
    };
    let minor = p.minor.num();
    let patch = p.patch.num();
    if let (Some(minor), Some(patch)) = (minor, patch) {
        // Fully concrete: the operator applies directly (an explicit `=`,
        // and a bare version with no operator at all, both mean `Op::Eq`).
        let version = concrete(major, minor, patch, &p.prerelease);
        return vec![Comp::Bound {
            op: op.unwrap_or(Op::Eq),
            version,
        }];
    }
    match op {
        Some(Op::Gt) => vec![Comp::Bound {
            op: Op::Gte,
            version: minor.map_or_else(
                || concrete(major + 1.0, 0.0, 0.0, &[]),
                |m| concrete(major, m + 1.0, 0.0, &[]),
            ),
        }],
        Some(Op::Gte) => vec![Comp::Bound {
            op: Op::Gte,
            version: concrete(major, minor.unwrap_or(0.0), 0.0, &[]),
        }],
        Some(Op::Lte) => vec![Comp::Bound {
            op: Op::Lt,
            version: minor.map_or_else(
                || concrete(major + 1.0, 0.0, 0.0, &[]),
                |m| concrete(major, m + 1.0, 0.0, &[]),
            ),
        }],
        Some(Op::Lt) => vec![Comp::Bound {
            op: Op::Lt,
            version: concrete(major, minor.unwrap_or(0.0), 0.0, &[]),
        }],
        // No operator (or `=` on a wildcard): an X-Range. `1.x` / `1` :=
        // `>=1.0.0 <2.0.0`; `1.2.x` / `1.2` := `>=1.2.0 <1.3.0`.
        Some(Op::Eq) | None => match minor {
            None => vec![
                Comp::Bound {
                    op: Op::Gte,
                    version: concrete(major, 0.0, 0.0, &[]),
                },
                Comp::Bound {
                    op: Op::Lt,
                    version: concrete(major + 1.0, 0.0, 0.0, &[]),
                },
            ],
            Some(m) => vec![
                Comp::Bound {
                    op: Op::Gte,
                    version: concrete(major, m, 0.0, &[]),
                },
                Comp::Bound {
                    op: Op::Lt,
                    version: concrete(major, m + 1.0, 0.0, &[]),
                },
            ],
        },
    }
}

/// Removes whitespace directly between a range operator (`<`, `<=`, `>`,
/// `>=`, `=`, `~`, `~>`, `^`) and the version that follows it, so `"> 1.2.3"`
/// and `">1.2.3"` tokenize identically (node-semver's `comparatorTrim`/
/// `tildeTrim`/`caretTrim` regexes, applied before splitting a range-set on
/// whitespace).
fn trim_after_operators(s: &str) -> String {
    let chars: Vec<char> = s.chars().collect();
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        out.push(c);
        let op_len = match c {
            '<' | '>' if chars.get(i + 1) == Some(&'=') => {
                out.push('=');
                2
            }
            '<' | '>' | '=' | '^' => 1,
            '~' if chars.get(i + 1) == Some(&'>') => {
                out.push('>');
                2
            }
            '~' => 1,
            _ => 0,
        };
        i += op_len.max(1);
        if op_len > 0 {
            while chars.get(i) == Some(&' ') {
                i += 1;
            }
        }
    }
    out
}

/// One comparator token (`^1.2.3`, `~1.2`, `>=1.0.0`, `1.2.x`, `1.2.3`, `*`,
/// ...): node-semver's `parseComparator` pipeline (caret, then tilde, then
/// X-range/plain), each firing on at most one prefix.
fn parse_comparator_token(token: &str) -> Option<Vec<Comp>> {
    if token.is_empty() || token == "*" {
        return Some(vec![Comp::Any]);
    }
    if let Some(rest) = token.strip_prefix('^') {
        return Some(caret_bounds(&parse_partial(rest)?));
    }
    if let Some(rest) = token.strip_prefix("~>").or_else(|| token.strip_prefix('~')) {
        return Some(tilde_bounds(&parse_partial(rest)?));
    }
    let (op, rest) = if let Some(r) = token.strip_prefix(">=") {
        (Some(Op::Gte), r)
    } else if let Some(r) = token.strip_prefix("<=") {
        (Some(Op::Lte), r)
    } else if let Some(r) = token.strip_prefix('>') {
        (Some(Op::Gt), r)
    } else if let Some(r) = token.strip_prefix('<') {
        (Some(Op::Lt), r)
    } else if let Some(r) = token.strip_prefix('=') {
        (Some(Op::Eq), r)
    } else {
        (None, token)
    };
    Some(xrange_bounds(op, &parse_partial(rest)?))
}

/// Whether `set` (trimmed, already whitespace-collapsed) is a hyphen range
/// (`"A - B"`, node-semver's `HYPHENRANGE`, which is anchored over the whole
/// range-set): the byte span around the first `" - "` must be the entire
/// string on both sides, with no second `" - "` left over in the tail.
fn split_hyphen(set: &str) -> Option<(&str, &str)> {
    let idx = set.find(" - ")?;
    let (from, rest) = set.split_at(idx);
    let to = &rest[3..];
    if from.trim().is_empty() || to.trim().is_empty() || to.contains(" - ") {
        return None;
    }
    Some((from.trim(), to.trim()))
}

/// node-semver `hyphenReplace`.
fn hyphen_bounds(from: &str, to: &str) -> Option<Vec<Comp>> {
    let f = parse_partial(from)?;
    let t = parse_partial(to)?;
    let mut comps = Vec::new();
    if let Some(fm) = f.major.num() {
        comps.push(Comp::Bound {
            op: Op::Gte,
            version: match (f.minor.num(), f.patch.num()) {
                (Some(fmin), Some(fp)) => concrete(fm, fmin, fp, &f.prerelease),
                (Some(fmin), None) => concrete(fm, fmin, 0.0, &[]),
                _ => concrete(fm, 0.0, 0.0, &[]),
            },
        });
    }
    if let Some(tm) = t.major.num() {
        comps.push(match (t.minor.num(), t.patch.num()) {
            (Some(tmin), Some(tp)) => Comp::Bound {
                op: Op::Lte,
                version: concrete(tm, tmin, tp, &t.prerelease),
            },
            (Some(tmin), None) => Comp::Bound {
                op: Op::Lt,
                version: concrete(tm, tmin + 1.0, 0.0, &[]),
            },
            (None, _) => Comp::Bound {
                op: Op::Lt,
                version: concrete(tm + 1.0, 0.0, 0.0, &[]),
            },
        });
    }
    if comps.is_empty() {
        comps.push(Comp::Any);
    }
    Some(comps)
}

/// One `||`-separated range-set: a hyphen range, or the AND of its
/// whitespace-separated comparator tokens.
fn parse_range_set(set: &str) -> Option<Vec<Comp>> {
    let set = set.trim();
    if set.is_empty() {
        return Some(vec![Comp::Any]);
    }
    if let Some((from, to)) = split_hyphen(set) {
        return hyphen_bounds(from, to);
    }
    let normalized = trim_after_operators(set);
    let mut comps = Vec::new();
    for token in normalized.split_whitespace() {
        comps.extend(parse_comparator_token(token)?);
    }
    if comps.is_empty() {
        comps.push(Comp::Any);
    }
    Some(comps)
}

/// node-semver's `Range` constructor: `None` when any `||`-branch has a
/// comparator this grammar cannot parse — the same as node-semver's own
/// (non-loose) `new Range(range)` throwing, which [`satisfies`] and
/// `minSatisfying` both catch and treat as "does not satisfy" (`try { range
/// = new Range(range, options) } catch (er) { return false }`).
fn parse_range(range: &str) -> Option<Vec<Vec<Comp>>> {
    let collapsed: String = range.split_whitespace().collect::<Vec<_>>().join(" ");
    collapsed.split("||").map(parse_range_set).collect()
}

/// The semver spec's version ordering: major, minor, patch, then
/// prerelease identifiers (a version with no prerelease outranks one that
/// has any; shared identifiers compare numeric-before-alphanumeric,
/// numerically or lexically, and a longer list outranks a shared prefix).
/// Build metadata plays no part, as the spec requires.
fn compare_semver(a: &SemVer, b: &SemVer) -> Ordering {
    a.major
        .total_cmp(&b.major)
        .then_with(|| a.minor.total_cmp(&b.minor))
        .then_with(|| a.patch.total_cmp(&b.patch))
        .then_with(|| compare_prerelease(&a.prerelease, &b.prerelease))
}

fn compare_prerelease(a: &[PrereleaseIdentifier], b: &[PrereleaseIdentifier]) -> Ordering {
    match (a.is_empty(), b.is_empty()) {
        (true, true) => Ordering::Equal,
        (true, false) => Ordering::Greater,
        (false, true) => Ordering::Less,
        (false, false) => a
            .iter()
            .zip(b.iter())
            .map(|(x, y)| compare_identifier(x, y))
            .find(|o| *o != Ordering::Equal)
            .unwrap_or_else(|| a.len().cmp(&b.len())),
    }
}

fn compare_identifier(a: &PrereleaseIdentifier, b: &PrereleaseIdentifier) -> Ordering {
    match (a, b) {
        (PrereleaseIdentifier::Number(x), PrereleaseIdentifier::Number(y)) => x.total_cmp(y),
        (PrereleaseIdentifier::Number(_), PrereleaseIdentifier::String(_)) => Ordering::Less,
        (PrereleaseIdentifier::String(_), PrereleaseIdentifier::Number(_)) => Ordering::Greater,
        (PrereleaseIdentifier::String(x), PrereleaseIdentifier::String(y)) => x.cmp(y),
    }
}

fn comp_test(c: &Comp, version: &SemVer) -> bool {
    match c {
        Comp::Any => true,
        Comp::Never => false,
        Comp::Bound { op, version: b } => {
            let ord = compare_semver(version, b);
            match op {
                Op::Eq => ord == Ordering::Equal,
                Op::Gt => ord == Ordering::Greater,
                Op::Gte => ord != Ordering::Less,
                Op::Lt => ord == Ordering::Less,
                Op::Lte => ord != Ordering::Greater,
            }
        }
    }
}

/// node-semver's `testSet`: every comparator in the set must match, and,
/// when the candidate itself carries a prerelease and `include_prerelease`
/// is not set, at least one comparator in the set must be a prerelease of
/// the exact same `[major, minor, patch]` (the "only the matching pre-
/// release window" carve-out `1.2.3-pr.2` needs against `^1.2.3-pr.1`).
fn test_set(set: &[Comp], version: &SemVer, include_prerelease: bool) -> bool {
    if !set.iter().all(|c| comp_test(c, version)) {
        return false;
    }
    if version.prerelease.is_empty() || include_prerelease {
        return true;
    }
    set.iter().any(|c| match c {
        Comp::Bound { version: b, .. } => {
            !b.prerelease.is_empty()
                && b.major == version.major
                && b.minor == version.minor
                && b.patch == version.patch
        }
        _ => false,
    })
}

/// A port of node-semver's `satisfies(version, range, {includePrerelease})`:
/// whether `version` (a plain `major.minor.patch` string; a `false` result
/// on `None` matches `semver.parse` failing and `satisfies` treating that as
/// "not satisfied") is within `range`.
pub(crate) fn satisfies(version: &str, range: &str, include_prerelease: bool) -> bool {
    let Some(v) = semver_parse(version) else {
        return false;
    };
    let Some(sets) = parse_range(range) else {
        return false;
    };
    sets.iter().any(|set| test_set(set, &v, include_prerelease))
}

#[cfg(test)]
mod tests {
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
}
