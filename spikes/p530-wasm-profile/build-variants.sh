#!/bin/sh
# P5-30: builds the spike's WASM variants and native binaries.
#
#   CARGO_TARGET_DIR=<per-task dir> sh build-variants.sh <out dir>
#
# Needs wasm-bindgen-cli 0.2.128 and wasm-opt (binaryen 132, from
# concerto-wasm's node_modules) on PATH, as concerto-wasm/build.sh does.
#
# <out dir>/wasm/<variant>/ holds a `wasm-bindgen --target nodejs` package;
# <out dir>/native/<variant> a native binary.
#
# WASM variants (all start from the shipped concerto-wasm settings:
# opt-level 3, fat LTO, one codegen unit, panic=abort, wasm-opt -O3 with
# build.sh's feature flags):
#   shipped    as shipped (the name section is dropped by wasm-opt)
#   named      shipped, plus wasm-opt -g: the name section kept, for profiles
#   no-wasmopt the raw rustc/wasm-bindgen output, no wasm-opt
#   simd128    RUSTFLAGS +simd128, wasm-opt with --enable-simd
#   talc       the talc allocator instead of std's dlmalloc
#   o2 / os    opt-level 2 / "s"
#   nolto      no LTO, 16 codegen units
# Native: glibc (default), dlmalloc (the wasm32 allocator), count-alloc.
set -eu
cd "$(dirname "$0")"
OUT=$(mkdir -p "$1" && cd "$1" && pwd)
T=${CARGO_TARGET_DIR:?set CARGO_TARGET_DIR}
FLAGS="--enable-bulk-memory --enable-nontrapping-float-to-int --enable-sign-ext --enable-reference-types --enable-multivalue --enable-mutable-globals"

wasm() {
  name=$1; profile=$2; features=$3; rustflags=$4; opt=$5
  tgt="$T/v-$name"
  RUSTFLAGS="$rustflags" CARGO_TARGET_DIR="$tgt" cargo build --profile "$profile" --target wasm32-unknown-unknown --features "wasm $features"
  dir=$profile; [ "$profile" = dev ] && dir=debug
  raw="$tgt/wasm32-unknown-unknown/$dir/p530_wasm_profile.wasm"
  d="$OUT/wasm/$name"; rm -rf "$d"; mkdir -p "$d"
  wasm-bindgen --target nodejs --out-dir "$d" "$raw"
  if [ "$opt" != none ]; then
    # shellcheck disable=SC2086
    wasm-opt $opt $FLAGS "$d/p530_wasm_profile_bg.wasm" -o "$d/opt.wasm"
    mv "$d/opt.wasm" "$d/p530_wasm_profile_bg.wasm"
  fi
  echo "$name: $(wc -c < "$d/p530_wasm_profile_bg.wasm") bytes"
}

wasm shipped    release       ""          ""                          "-O3"
wasm named      release       ""          ""                          "-O3 -g"
wasm no-wasmopt release       ""          ""                          none
wasm simd128    release       ""          "-C target-feature=+simd128" "-O3 --enable-simd"
wasm talc       release       wasm-talc   ""                          "-O3"
wasm o2         release-o2    ""          ""                          "-O3"
wasm os         release-os    ""          ""                          "-O3"
wasm nolto      release-nolto ""          ""                          "-O3"

mkdir -p "$OUT/native"
native() {
  name=$1; features=$2
  tgt="$T/n-$name"
  CARGO_TARGET_DIR="$tgt" cargo build --release --bin p530-native --features "$features"
  cp "$tgt/release/p530-native" "$OUT/native/$name"
}
native glibc ""
native dlmalloc native-dlmalloc
native count-alloc count-alloc
echo "build-variants: done"
