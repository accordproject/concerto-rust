#!/bin/sh
# P5-30 (accordproject/concerto-rust#335): the timed and profiled runs of
# the spike. Measure only.
#
#   sh p530-run.sh <phase> <out dir>
#
# Environment:
#   CONCERTO    a concerto checkout with a built concerto-core dist/ and the
#               oracle reference installed (migration/oracle/reference).
#   ENGINE      the shipped engine, concerto-wasm/pkg/concerto-engine.cjs.
#   ENGINE_NAMED  the same engine rebuilt with symbols (a wasm-bindgen
#               nodejs dir holding concerto_wasm.js), for its profile.
#   V           build-variants.sh's output dir.
#   DATA        the dumped `models` inputs, <set>.json (scripts/dump-engine.cjs).
#   SELF        a pattern naming this run's own processes, which the quiet
#               gate ignores (default P5-30).
#
# phase:
#   profiles  V8 CPU profiles of the symbolised spike (the whole binding body
#             and extract alone) and of the symbolised engine, on
#             synthetic-large. No gate (shares only, not times).
#   timed     three interleaved rounds, each part behind the P5-15/P5-22
#             quiet gate (1-min load < 2, 5-min load < 3, no other bench,
#             cargo or mocha process): TS 5.0.0 and the Rust engine through
#             the TS API (p515-sweep.mjs, extract_decorators), the native
#             variants, and the WASM variants (plus node flag variants).
set -u
PHASE=$1
OUT=$(mkdir -p "$2" && cd "$2" && pwd)
HERE=$(cd "$(dirname "$0")" && pwd)
SETS="synthetic-large conformance concerto-core-test-data"
REF=$CONCERTO/migration/oracle/reference/node_modules/@accordproject/concerto-core/dist

loads() { awk '{print $1" "$2" "$3}' /proc/loadavg; }

if [ "$PHASE" = profiles ]; then
  mkdir -p "$OUT/prof"
  for loop in all extract; do
    d="$OUT/prof/spike-$loop"; rm -rf "$d"; mkdir -p "$d"
    node --cpu-prof --cpu-prof-dir="$d" --cpu-prof-interval 100 "$HERE/scripts/run-wasm.mjs" \
      --pkg "$V/wasm/named" --input "$DATA/synthetic-large.json" --loop "$loop" --seconds 20 2> "$d/loop.log"
    node "$HERE/scripts/summarize.mjs" cpuprof "$d"/*.cpuprofile --top 60 > "$OUT/prof/spike-$loop.json"
    echo "profile spike-$loop done ($(loads))"
  done
  d="$OUT/prof/engine"; rm -rf "$d"; mkdir -p "$d"
  node --cpu-prof --cpu-prof-dir="$d" --cpu-prof-interval 100 "$HERE/scripts/run-wasm.mjs" \
    --engine "$ENGINE_NAMED/concerto_wasm.js" --input "$DATA/synthetic-large.json" --loop engine --seconds 20 2> "$d/loop.log"
  node "$HERE/scripts/summarize.mjs" cpuprof "$d"/*.cpuprofile --top 60 > "$OUT/prof/engine.json"
  echo "profile engine done ($(loads))"
  echo "profiles done"
  exit 0
fi

if [ "$PHASE" != timed ]; then echo "unknown phase $PHASE"; exit 2; fi

MAX=${P530_GATE_MAX:-21600}
waited=0
gate() {
  while :; do
    set -- $(loads)
    busy=$(pgrep -af 'run-ts\.mjs|p5[0-9a-z]*-(sweep|rounds|profile)|--bench|criterion|wasm-instance|(^|/)cargo( |$)|rustc|mocha|valgrind' \
      | grep -v "${SELF:-P5-30}" | grep -v pgrep | wc -l | tr -d ' ')
    if awk -v a="$1" -v b="$2" 'BEGIN{exit !(a<2 && b<3)}' && [ "$busy" = 0 ]; then
      echo "gate ok after ${waited}s: load $1 $2 $3" >> "$OUT/timed-loads.txt"
      return 0
    fi
    if [ $waited -ge "$MAX" ]; then
      echo "gate: gave up after ${waited}s (load $1 $2 $3, other processes $busy)"
      exit 20
    fi
    sleep 30; waited=$((waited+30))
  done
}
part() { # name, command...
  name=$1; shift
  gate
  echo "$name start $(loads)" >> "$OUT/timed-loads.txt"
  "$@"
  echo "$name end $(loads)" >> "$OUT/timed-loads.txt"
}

ts_api() {
  r=$1
  (cd "$CONCERTO" && node migration/bench/p515-sweep.mjs --core-dist "$REF" --ops extract_decorators --samples 30 --warmup 5 \
    --out "$OUT/r$r/ts-reference-5.0.0.json" > "$OUT/r$r/ts-reference-5.0.0.log" 2>&1)
  (cd "$CONCERTO" && CONCERTO_ENGINE_MODULE="$ENGINE" node migration/bench/p515-sweep.mjs --ops extract_decorators --samples 30 --warmup 5 \
    --out "$OUT/r$r/rust-engine.json" > "$OUT/r$r/rust-engine.log" 2>&1)
}
natives() {
  r=$1
  for n in glibc dlmalloc; do
    for s in $SETS; do
      "$V/native/$n" "$DATA/$s.json" --iters 30 --warmup 5 > "$OUT/r$r/native-$n-$s.json"
    done
  done
  if [ "$r" = 1 ]; then
    for s in $SETS; do
      "$V/native/count-alloc" "$DATA/$s.json" --iters 3 --warmup 1 > "$OUT/r$r/native-count-alloc-$s.json"
    done
  fi
}
wasms() {
  r=$1
  for v in shipped no-wasmopt simd128 talc o2 os nolto; do
    for s in $SETS; do
      extra=""
      [ "$v" = shipped ] && extra="--engine $ENGINE"
      # shellcheck disable=SC2086
      node "$HERE/scripts/run-wasm.mjs" --pkg "$V/wasm/$v" --input "$DATA/$s.json" --iters 30 --warmup 5 $extra \
        > "$OUT/r$r/wasm-$v-$s.json"
    done
  done
  for f in no-liftoff wasm-enforce-bounds-checks no-wasm-bounds-checks experimental-wasm-inlining; do
    node "--$f" "$HERE/scripts/run-wasm.mjs" --pkg "$V/wasm/shipped" --input "$DATA/synthetic-large.json" --iters 30 --warmup 5 \
      --engine "$ENGINE" > "$OUT/r$r/wasm-flag-$f-synthetic-large.json"
  done
}

for r in 1 2 3; do
  mkdir -p "$OUT/r$r"
  part "round $r ts-api" ts_api "$r"
  if [ $((r % 2)) = 1 ]; then
    part "round $r native" natives "$r"
    part "round $r wasm" wasms "$r"
  else
    part "round $r wasm" wasms "$r"
    part "round $r native" natives "$r"
  fi
done
echo "timed done"
