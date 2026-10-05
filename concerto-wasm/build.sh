#!/bin/sh
# Builds the concerto-wasm binding and writes the loaders into pkg/:
#
#   pkg/concerto_wasm.wasm        the optimised module, a raw .wasm file
#   pkg/concerto-engine.cjs       CommonJS (Node `require`): the `nodejs`
#                                 glue, reading concerto_wasm.wasm
#   pkg/concerto-engine.node.mjs  ESM for Node: the `web` glue, instantiated
#                                 with `initSync` from concerto_wasm.wasm
#   pkg/concerto-engine.mjs       ESM for browsers: the `web` glue,
#                                 instantiated with `initSync` from the
#                                 .wasm inlined as base64
#
# All three instantiate synchronously at load (spike REPORT §1); Node reads
# the raw .wasm with readFileSync (P5-44). pkg/ is the npm
# package @accordproject/concerto-engine, which the concerto checkout links
# locally (packages/concerto-engine); it is not published (decision D9).
#
# Needs the wasm32-unknown-unknown target and wasm-bindgen-cli 0.2.128 (the
# exact wasm-bindgen crate version). wasm-opt (binaryen) is used when it is on
# PATH; `npm run build` puts the pinned npm binaryen there. jq is needed only
# to resolve the target directory via `cargo metadata` when CARGO_TARGET_DIR
# is unset; without jq, the build falls back to the plain `target` dir.
#
# The build fails when the optimised module is over the size budget,
# BUDGET bytes (spike REPORT §2: Chromium compiles at most 8 MiB
# synchronously on the main thread; the budget keeps half of that in hand).
set -eu
cd "$(dirname "$0")"

NAME=concerto_wasm
BUDGET=4194304

cargo build --release --target wasm32-unknown-unknown

# Resolve the build's target directory the same way cargo did: honour
# CARGO_TARGET_DIR when it's set (the worker workflows' no-shared-target-dir
# rule redirects it), otherwise ask cargo (needs jq), which also accounts for
# a target-dir set in .cargo/config.toml, falling back to plain `target`
# (relative to this directory, since we already cd'd above) when jq isn't on
# PATH. Without this, RAW below could point at a stale or missing ./target
# file while cargo actually wrote elsewhere.
if [ -n "${CARGO_TARGET_DIR:-}" ]; then
  TARGET_DIR="$CARGO_TARGET_DIR"
elif command -v jq >/dev/null 2>&1; then
  TARGET_DIR="$(cargo metadata --format-version 1 --no-deps | jq -r .target_directory)"
else
  TARGET_DIR="target"
fi
RAW="$TARGET_DIR/wasm32-unknown-unknown/release/$NAME.wasm"

rm -rf pkg
wasm-bindgen --target web    --out-dir pkg/web  "$RAW"
wasm-bindgen --target nodejs --out-dir pkg/node "$RAW"

if command -v wasm-opt >/dev/null 2>&1; then
  # Rust's wasm32 target enables these proposals by default. -O3, not -Oz:
  # optimised for speed (P5-06), like the release profile.
  wasm-opt -O3 --enable-bulk-memory --enable-nontrapping-float-to-int --enable-sign-ext \
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
