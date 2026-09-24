#!/bin/sh
# Builds the concerto-wasm binding and writes the loaders into pkg/:
#
#   pkg/concerto-engine.cjs  CommonJS (Node `require`): the `nodejs` glue with
#                            the .wasm inlined as base64
#   pkg/concerto-engine.mjs  ESM (browsers, Node ESM): the `web` glue,
#                            instantiated with `initSync` from inlined bytes
#
# Both instantiate synchronously at load (spike REPORT §1). pkg/ is the npm
# package @accordproject/concerto-engine, which the concerto checkout links
# locally (packages/concerto-engine); it is not published (decision D9).
#
# Needs the wasm32-unknown-unknown target and wasm-bindgen-cli 0.2.128 (the
# exact wasm-bindgen crate version). wasm-opt (binaryen) is used when it is on
# PATH; `npm run build` puts the pinned npm binaryen there.
#
# The build fails when the optimised module is over the size budget,
# BUDGET bytes (spike REPORT §2: Chromium compiles at most 8 MiB
# synchronously on the main thread; the budget keeps half of that in hand).
set -eu
cd "$(dirname "$0")"

NAME=concerto_wasm
RAW="target/wasm32-unknown-unknown/release/$NAME.wasm"
BUDGET=4194304

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

SIZE=$(wc -c < "pkg/${NAME}.wasm" | tr -d ' ')
if [ "$SIZE" -gt "$BUDGET" ]; then
  echo "build.sh: pkg/${NAME}.wasm is $SIZE bytes, over the $BUDGET-byte budget" >&2
  exit 1
fi
echo "build.sh: pkg/${NAME}.wasm is $SIZE bytes (budget $BUDGET)"
