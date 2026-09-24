#!/bin/sh
# Builds the spike with cargo + wasm-bindgen-cli + wasm-opt, then writes the
# inlined-bytes loaders. Needs: the wasm32-unknown-unknown target,
# wasm-bindgen-cli matching the wasm-bindgen crate (0.2.128), and `npm install`
# here (for binaryen's wasm-opt and Playwright).
#
#   sh build.sh              # size-optimised release (opt-level "z")
#   PROFILE=release-speed sh build.sh
set -eu
cd "$(dirname "$0")"

PROFILE="${PROFILE:-release}"
NAME=concerto_wasm_spike
RAW="target/wasm32-unknown-unknown/$PROFILE/$NAME.wasm"

cargo build --profile "$PROFILE" --target wasm32-unknown-unknown

rm -rf dist
wasm-bindgen --target web    --out-dir dist/web  "$RAW"
wasm-bindgen --target nodejs --out-dir dist/node "$RAW"

cp "dist/web/${NAME}_bg.wasm" "dist/${NAME}_bindgen.wasm"

# Rust's wasm32 target enables these proposals by default, so wasm-opt must too.
FEATURES="--enable-bulk-memory --enable-nontrapping-float-to-int --enable-sign-ext \
  --enable-reference-types --enable-multivalue --enable-mutable-globals"
# shellcheck disable=SC2086
node_modules/.bin/wasm-opt -Oz $FEATURES "dist/web/${NAME}_bg.wasm" -o "dist/${NAME}_opt.wasm"
cp "dist/${NAME}_opt.wasm" "dist/web/${NAME}_bg.wasm"
cp "dist/${NAME}_opt.wasm" "dist/node/${NAME}_bg.wasm"

node scripts/inline.mjs
node scripts/sizes.mjs "$RAW" > dist/sizes.txt
cat dist/sizes.txt
