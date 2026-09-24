#!/bin/sh
# Builds the concerto-wasm binding and writes the loaders into pkg/:
#
#   pkg/concerto-engine.cjs  CommonJS (Node `require`): the `nodejs` glue with
#                            the .wasm inlined as base64
#   pkg/concerto-engine.mjs  ESM (browsers, Node ESM): the `web` glue,
#                            instantiated with `initSync` from inlined bytes
#
# Both instantiate synchronously at load (spike REPORT §1). Needs the
# wasm32-unknown-unknown target and wasm-bindgen-cli 0.2.128 (the exact
# wasm-bindgen crate version). wasm-opt (binaryen) is used when it is on PATH.
#
# P0-04b trial scaffold: P4-01 owns the packaging (npm name, CI, size budget).
set -eu
cd "$(dirname "$0")"

NAME=concerto_wasm
RAW="target/wasm32-unknown-unknown/release/$NAME.wasm"

cargo build --release --target wasm32-unknown-unknown

rm -rf pkg
wasm-bindgen --target web    --out-dir pkg/web  "$RAW"
wasm-bindgen --target nodejs --out-dir pkg/node "$RAW"

if command -v wasm-opt >/dev/null 2>&1; then
  # Rust's wasm32 target enables these proposals by default.
  wasm-opt -Oz --enable-bulk-memory --enable-nontrapping-float-to-int --enable-sign-ext \
    --enable-reference-types --enable-multivalue --enable-mutable-globals \
    "pkg/web/${NAME}_bg.wasm" -o "pkg/${NAME}.wasm"
else
  echo "build.sh: wasm-opt not found; the module is not size-optimised" >&2
  cp "pkg/web/${NAME}_bg.wasm" "pkg/${NAME}.wasm"
fi

node scripts/inline.mjs
