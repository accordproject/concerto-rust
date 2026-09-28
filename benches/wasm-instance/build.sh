#!/bin/sh
# Builds the in-WASM workload 3 bench into pkg/ (see README.md): the same
# wasm32 release build, wasm-bindgen and wasm-opt -O3 as concerto-wasm's
# build.sh, with the Node glue only.
#
# Needs the wasm32-unknown-unknown target, wasm-bindgen-cli 0.2.128 and
# wasm-opt on PATH (concerto-wasm's `npm ci` installs the pinned binaryen
# under concerto-wasm/node_modules/.bin).
set -eu
cd "$(dirname "$0")"

NAME=concerto_bench_wasm_instance
TARGET_DIR="${CARGO_TARGET_DIR:-target}"
RAW="$TARGET_DIR/wasm32-unknown-unknown/release/$NAME.wasm"

cargo build --release --target wasm32-unknown-unknown

rm -rf pkg
wasm-bindgen --target nodejs --out-dir pkg "$RAW"
if ! command -v wasm-opt >/dev/null 2>&1; then
  echo "build.sh: wasm-opt not found; put binaryen's wasm-opt on PATH" >&2
  exit 1
fi
wasm-opt -O3 --enable-bulk-memory --enable-nontrapping-float-to-int --enable-sign-ext \
  --enable-reference-types --enable-multivalue --enable-mutable-globals \
  "pkg/${NAME}_bg.wasm" -o "pkg/${NAME}_bg.opt.wasm"
mv "pkg/${NAME}_bg.opt.wasm" "pkg/${NAME}_bg.wasm"
echo "build.sh: pkg/${NAME}_bg.wasm is $(wc -c < "pkg/${NAME}_bg.wasm" | tr -d ' ') bytes"
