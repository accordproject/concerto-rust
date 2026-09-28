#!/usr/bin/env bash
# P6-03 (accordproject/concerto-rust#85, docs/public-api.md section 2.1 and
# 4.6): a `cargo public-api` snapshot of `accordproject-concerto-core`, built
# with default features (js-compat off), so the check sees exactly the D11
# stable surface.
#
# Usage:
#   scripts/check-public-api.sh --check   # CI: fail if the snapshot is stale
#   scripts/check-public-api.sh --bless   # update the committed snapshot
#
# Requires a nightly toolchain (cargo-public-api builds rustdoc JSON, which
# is nightly-only) and `cargo install cargo-public-api --locked`.
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
snapshot="$repo_root/concerto-core/public-api.txt"
mode="${1:---check}"

if ! command -v cargo-public-api >/dev/null 2>&1; then
  echo "error: cargo-public-api is not installed (cargo install cargo-public-api --locked)" >&2
  exit 1
fi

if ! rustup toolchain list | grep -q '^nightly'; then
  echo "error: a nightly toolchain is required to build rustdoc JSON (rustup toolchain install nightly)" >&2
  exit 1
fi

current="$(mktemp)"
trap 'rm -f "$current"' EXIT

# Default features only (js-compat is off): this is the D11 stable surface,
# not the seam concerto-wasm and concerto-core-js build on
# (docs/public-api.md section 2.1, item 1, and section 4.6). cargo-public-api
# has no --toolchain flag; it shells out to `cargo rustdoc`, which needs a
# nightly default for the unstable `--output-format json` flag, so run it
# through `rustup run nightly`.
rustup run nightly cargo public-api \
  --manifest-path "$repo_root/concerto-core/Cargo.toml" \
  --color=never \
  --simplified \
  >"$current"

# docs/public-api.md section 2 item 7 / section 4.6: no JS-facing type may
# leak into the default surface, whatever features are enabled elsewhere in
# the workspace graph, since cargo features are additive.
banned_pattern='wasm_bindgen|js_sys|JsValue|SerializerOptions|Dayjs|ts_class|\$\$'
if grep -nE "$banned_pattern" "$current"; then
  echo "error: a JS-facing name leaked into the default public API of accordproject-concerto-core (docs/public-api.md section 4.6)" >&2
  exit 1
fi

case "$mode" in
  --bless)
    cp "$current" "$snapshot"
    echo "Updated $snapshot"
    ;;
  --check)
    if [ ! -f "$snapshot" ]; then
      echo "error: no committed snapshot at $snapshot (run with --bless first)" >&2
      exit 1
    fi
    if ! diff -u "$snapshot" "$current"; then
      echo >&2
      echo "error: the public API of accordproject-concerto-core changed (default features)." >&2
      echo "If this is deliberate, review it against docs/public-api.md section 2 (the semver" >&2
      echo "guarantee) and section 5.8 (the deprecation policy), bump the version if it is" >&2
      echo "breaking, then run 'scripts/check-public-api.sh --bless' and commit the result." >&2
      exit 1
    fi
    echo "OK: accordproject-concerto-core's public API matches $snapshot"
    ;;
  *)
    echo "usage: $0 [--check|--bless]" >&2
    exit 2
    ;;
esac
