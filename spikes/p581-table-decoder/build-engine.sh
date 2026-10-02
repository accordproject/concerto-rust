#!/bin/sh
# P5-81 (accordproject/concerto-rust#425): concerto-wasm's build.sh with
# cargo features, for the speed comparison and the mocha/smoke runs of the
# table decoder. The same steps (opt-level 3, wasm-bindgen web + nodejs,
# wasm-opt -O3, scripts/inline.mjs); build.sh itself takes no features and
# is left unchanged. Writes concerto-wasm/pkg/ (as build.sh does), then
# copies it to <out dir>.
#
#   sh build-engine.sh <out dir> [<cargo features>]
#
# CARGO_TARGET_DIR must be set; WASM_OPT names the binaryen wasm-opt.
set -eu
OUT=$1
FEATURES=${2:-}
: "${CARGO_TARGET_DIR:?set CARGO_TARGET_DIR}"
WASM_OPT=${WASM_OPT:-wasm-opt}
HERE=$(cd "$(dirname "$0")" && pwd)
mkdir -p "$OUT"
OUT=$(cd "$OUT" && pwd)
cd "$HERE/../../concerto-wasm"
NAME=concerto_wasm
cargo build --release --target wasm32-unknown-unknown ${FEATURES:+--features "$FEATURES"}
RAW="$CARGO_TARGET_DIR/wasm32-unknown-unknown/release/$NAME.wasm"
rm -rf pkg
wasm-bindgen --target web    --out-dir pkg/web  "$RAW"
wasm-bindgen --target nodejs --out-dir pkg/node "$RAW"
"$WASM_OPT" -O3 --enable-bulk-memory --enable-nontrapping-float-to-int --enable-sign-ext \
  --enable-reference-types --enable-multivalue --enable-mutable-globals \
  "pkg/web/${NAME}_bg.wasm" -o "pkg/${NAME}.wasm"
node scripts/inline.mjs
rm -rf "$OUT"; mkdir -p "$OUT"; cp -R pkg/. "$OUT/"
echo "build-engine.sh: $(wc -c < "pkg/${NAME}.wasm" | tr -d ' ') bytes -> $OUT"
