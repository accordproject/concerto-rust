#!/bin/sh
# P5-81 (accordproject/concerto-rust#425): builds the concerto-wasm engine for
# the size comparison, the way build.sh does, plus a named copy for the
# per-area breakdown (P5-39/P5-48 method).
#
#   sh build-wasm.sh <out dir> <opt-level: 3|z> [<cargo features>]
#
# Writes <out>/shipped.wasm (wasm-opt -O3 for opt-level 3, -Oz for z; names
# stripped, as build.sh ships it) and <out>/named.wasm (the same with -g, so
# the name section survives for p548-wasmsize.mjs). CARGO_TARGET_DIR must be
# set (one per opt-level; never shared between worktrees). WASM_OPT names the
# binaryen wasm-opt (build.sh's pinned npm binaryen 132).
set -eu
OUT=$1
OPT=$2
FEATURES=${3:-}
: "${CARGO_TARGET_DIR:?set CARGO_TARGET_DIR}"
WASM_OPT=${WASM_OPT:-wasm-opt}
HERE=$(cd "$(dirname "$0")" && pwd)
mkdir -p "$OUT"
OUT=$(cd "$OUT" && pwd)
cd "$HERE/../../concerto-wasm"
CARGO_PROFILE_RELEASE_OPT_LEVEL=$OPT CARGO_PROFILE_RELEASE_STRIP=false \
  cargo build --release --target wasm32-unknown-unknown ${FEATURES:+--features "$FEATURES"}
RAW="$CARGO_TARGET_DIR/wasm32-unknown-unknown/release/concerto_wasm.wasm"
rm -rf "$OUT/web"
wasm-bindgen --target web --out-dir "$OUT/web" "$RAW"
case "$OPT" in
  3) LEVEL=-O3 ;;
  *) LEVEL=-Oz ;;
esac
FLAGS="--enable-bulk-memory --enable-nontrapping-float-to-int --enable-sign-ext --enable-reference-types --enable-multivalue --enable-mutable-globals"
"$WASM_OPT" $LEVEL $FLAGS "$OUT/web/concerto_wasm_bg.wasm" -o "$OUT/shipped.wasm"
"$WASM_OPT" $LEVEL -g $FLAGS "$OUT/web/concerto_wasm_bg.wasm" -o "$OUT/named.wasm"
ls -l "$OUT/shipped.wasm" "$OUT/named.wasm"
