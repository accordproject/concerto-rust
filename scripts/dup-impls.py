#!/usr/bin/env python3
"""Duplicate-implementation detector for concerto-rust.

The plan (accordproject/concerto-rust#29, task P1-03) sets two thresholds:

  1. a method implemented on more than 2 types must come from a trait;
  2. a trait implemented by hand more than 3 times must have a derive in
     `concerto-macros`, and its repeated impls must use it.

This script checks both over the non-test Rust sources it is given (by
default `concerto-core/src`). It reads the source text only, so it needs no
build and no third-party packages.

  Rule 1 (`trait`): an inherent method that takes `self` and is defined, under
  the same name, on more than 2 types. Associated functions without a
  receiver (constructors such as `from_json`) are not counted: they take
  different arguments per type, so they are not one method implemented
  several times.

  Rule 2 (`derive`): a trait defined in the scanned sources and implemented by
  hand (`impl Trait for Type`) on more than 3 types. Impls written by a derive
  (`#[derive(Trait)]`) do not count.

Test modules (`#[cfg(test)] mod ... { ... }`) and `macro_rules!` bodies are
skipped: a `macro_rules!` body is a template, and what it expands to is
generated code, not a hand-written duplicate.

Hand-built error values (`ConcertoError::IllegalModel { .. }` and
`ConcertoError::ValidationFailed { .. }`) are listed separately and never fail
the check: the error builder that replaces them belongs to P1-05
(accordproject/concerto-rust#42).

Usage: scripts/dup-impls.py [--verbose] [PATH ...]
Exit status: 0 when nothing is over a threshold, 1 otherwise.
"""

import os
import re
import sys
from collections import defaultdict

INHERENT_LIMIT = 2  # more than this many types need a trait
HAND_IMPL_LIMIT = 3  # more than this many hand-written impls need a derive
ERROR_CONSTRUCTIONS = ("IllegalModel", "ValidationFailed")


def strip_comments_and_literals(text):
    """Blanks out comments and the contents of string and char literals,
    keeping every newline so that line numbers still match the file."""
    out = []
    i, n = 0, len(text)
    while i < n:
        c = text[i]
        nxt = text[i + 1] if i + 1 < n else ""
        if c == "/" and nxt == "/":
            while i < n and text[i] != "\n":
                i += 1
            continue
        if c == "/" and nxt == "*":
            depth, i = 1, i + 2
            while i < n and depth:
                if text.startswith("/*", i):
                    depth, i = depth + 1, i + 2
                elif text.startswith("*/", i):
                    depth, i = depth - 1, i + 2
                else:
                    if text[i] == "\n":
                        out.append("\n")
                    i += 1
            continue
        raw = re.match(r'b?r(#*)"', text[i:]) if c in "rb" else None
        if raw and (i == 0 or not (text[i - 1].isalnum() or text[i - 1] == "_")):
            end = '"' + raw.group(1)
            j = text.find(end, i + raw.end())
            j = n if j < 0 else j + len(end)
            out.append('""' + "\n" * text.count("\n", i, j))
            i = j
            continue
        if c == '"':
            j = i + 1
            while j < n and text[j] != '"':
                j += 2 if text[j] == "\\" else 1
            out.append('""' + "\n" * text.count("\n", i, j))
            i = j + 1
            continue
        if c == "'":
            # A char literal ('x', '\n', '\u{..}'), not a lifetime ('a).
            lit = re.match(r"'(\\u\{[0-9a-fA-F]+\}|\\.|[^\\'])'", text[i:])
            if lit:
                out.append("' '")
                i += lit.end()
                continue
        out.append(c)
        i += 1
    return "".join(out)


def block_end(text, open_brace):
    """The index just past the `}` that closes the `{` at `open_brace`."""
    depth = 0
    for j in range(open_brace, len(text)):
        if text[j] == "{":
            depth += 1
        elif text[j] == "}":
            depth -= 1
            if depth == 0:
                return j + 1
    return len(text)


def blank(text, start, end):
    """Replaces text[start:end] with spaces, keeping newlines."""
    return text[:start] + re.sub(r"[^\n]", " ", text[start:end]) + text[end:]


def drop_tests_and_macro_bodies(text):
    for pattern in (
        r"#\[cfg\(test\)\]\s*(pub(\([^)]*\))?\s+)?mod\s+\w+\s*\{",
        r"macro_rules!\s*\w+\s*\{",
    ):
        while True:
            m = re.search(pattern, text)
            if not m:
                break
            text = blank(text, m.start(), block_end(text, m.end() - 1))
    return text


def base_name(path):
    """`crate::a::Foo<'a, T>` -> `Foo`; `&[T]` and other odd types unchanged."""
    path = path.strip()
    depth, cut = 0, len(path)
    for k, ch in enumerate(path):
        if ch == "<":
            if depth == 0:
                cut = k
                break
            depth += 1
    head = path[:cut].strip()
    return head.split("::")[-1] if head else path


def split_impl_header(header):
    """`impl<..> Trait for Type where ..` -> (trait or None, type)."""
    header = re.sub(r"^impl\s*", "", header.strip())
    if header.startswith("<"):
        depth = 0
        for k, ch in enumerate(header):
            depth += (ch == "<") - (ch == ">")
            if depth == 0:
                header = header[k + 1 :]
                break
    header = re.split(r"\bwhere\b", header)[0].strip()
    depth = 0
    for k in range(len(header)):
        ch = header[k]
        depth += (ch == "<") - (ch == ">")
        if depth == 0 and header.startswith(" for ", k):
            return base_name(header[:k].lstrip("!")), base_name(header[k + 5 :])
    return None, base_name(header)


def self_methods(body):
    """Names of the `fn`s at the top level of an impl body that take `self`."""
    names = []
    depth = 0
    k = 0
    while k < len(body):
        ch = body[k]
        if ch == "{":
            depth += 1
        elif ch == "}":
            depth -= 1
        elif depth == 1 and body.startswith("fn", k) and (k == 0 or not body[k - 1].isalnum()):
            m = re.match(r"fn\s+(\w+)\s*(<[^(]*>)?\s*\(\s*([^,)]*)", body[k:])
            if m and re.fullmatch(r"(&\s*('\w+\s+)?)?(mut\s+)?self(\s*:.*)?", m.group(3).strip()):
                names.append(m.group(1))
        k += 1
    return names


def scan(paths):
    inherent = defaultdict(set)  # method -> {type}
    hand_impls = defaultdict(list)  # trait -> [(type, location)]
    derived = defaultdict(list)  # derive name -> [location]
    local_traits = set()
    errors = []  # (location, variant)

    files = []
    for root in paths:
        if os.path.isfile(root):
            files.append(root)
            continue
        for dirpath, _, names in os.walk(root):
            files += [os.path.join(dirpath, f) for f in names if f.endswith(".rs")]

    for path in sorted(files):
        with open(path, encoding="utf-8") as fh:
            text = drop_tests_and_macro_bodies(strip_comments_and_literals(fh.read()))

        def where(offset):
            return f"{path}:{text.count(chr(10), 0, offset) + 1}"

        for m in re.finditer(r"\b(pub(\([^)]*\))?\s+)?trait\s+(\w+)", text):
            local_traits.add(m.group(3))
        for m in re.finditer(r"#\[derive\(([^)]*)\)\]", text):
            for name in m.group(1).split(","):
                if name.strip():
                    derived[base_name(name)].append(where(m.start()))
        for m in re.finditer(r"ConcertoError::(" + "|".join(ERROR_CONSTRUCTIONS) + r")\s*\{", text):
            errors.append((where(m.start()), m.group(1)))
        for m in re.finditer(r"(?<![\w])impl\b", text):
            brace = text.find("{", m.start())
            semi = text.find(";", m.start())
            if brace < 0 or (0 <= semi < brace):
                continue
            trait, ty = split_impl_header(text[m.start() : brace])
            body = text[brace : block_end(text, brace)]
            if trait is None:
                for method in self_methods(body):
                    inherent[method].add(ty)
            else:
                hand_impls[trait].append((ty, where(m.start())))
    return inherent, hand_impls, derived, local_traits, errors


def main(argv):
    verbose = "--verbose" in argv
    paths = [a for a in argv if not a.startswith("--")]
    if not paths:
        here = os.path.dirname(os.path.abspath(__file__))
        paths = [os.path.relpath(os.path.join(here, "..", "concerto-core", "src"))]
    inherent, hand_impls, derived, local_traits, errors = scan(paths)

    failures = []
    for method, types in sorted(inherent.items()):
        if len(types) > INHERENT_LIMIT:
            failures.append(
                f"trait: `{method}(&self..)` is implemented on {len(types)} types "
                f"without a trait: {', '.join(sorted(types))}"
            )
    for trait in sorted(local_traits):
        impls = hand_impls.get(trait, [])
        if len(impls) > HAND_IMPL_LIMIT:
            failures.append(
                f"derive: `{trait}` has {len(impls)} hand-written impls "
                f"({', '.join(ty for ty, _ in impls)}); derive the repeated ones"
            )

    print(f"thresholds: >{INHERENT_LIMIT} types per inherent method, "
          f">{HAND_IMPL_LIMIT} hand-written impls per local trait")
    print("\nlocal traits (hand-written impls / derived impls):")
    for trait in sorted(local_traits):
        impls = hand_impls.get(trait, [])
        print(f"  {trait}: {len(impls)} by hand, {len(derived.get(trait, []))} derived")
        if verbose:
            for ty, loc in impls:
                print(f"      {ty}  {loc}")
    if verbose:
        print("\ninherent methods on more than one type:")
        for method, types in sorted(inherent.items()):
            if len(types) > 1:
                print(f"  {method}: {', '.join(sorted(types))}")

    print(f"\nexcluded, left for P1-05 (#42): {len(errors)} hand-built error values")
    for variant in ERROR_CONSTRUCTIONS:
        sites = [loc for loc, v in errors if v == variant]
        print(f"  ConcertoError::{variant} {{..}}: {len(sites)}")
        for loc in sites:
            print(f"      {loc}")

    if failures:
        print("\nFAIL")
        for failure in failures:
            print(f"  {failure}")
        return 1
    print("\nOK: nothing over the thresholds")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
