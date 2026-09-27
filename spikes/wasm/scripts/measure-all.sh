#!/bin/sh
# Rebuilds both profiles and records every measurement under results/.
# NODES may list extra node binaries for the cold-load table.
set -eu
cd "$(dirname "$0")/.."
mkdir -p results
NODES="${NODES:-$(command -v node)}"
for P in release release-speed; do
  PROFILE=$P sh build.sh > /dev/null 2>&1
  cp dist/sizes.txt "results/$P-sizes.txt"
  node scripts/dump-set.mjs
  cargo run -q --profile "$P" --example native > "results/$P-native.txt"
  # shellcheck disable=SC2086
  node scripts/load-bench.mjs $NODES > "results/$P-node-load.txt"
  node scripts/bench.mjs --runs 5 > "results/$P-node-bench.txt"
  node scripts/chromium-smoke.mjs --bench > "results/$P-chromium.txt"
done
# Leave dist/ holding the size-optimised build, and record the smoke runs.
PROFILE=release sh build.sh > /dev/null 2>&1
for N in $NODES; do
  V=$("$N" --version)
  "$N" scripts/node-smoke.mjs > "results/smoke-node-$V-esm.txt"
  "$N" scripts/node-smoke.cjs > "results/smoke-node-$V-cjs.txt"
done
node scripts/chromium-smoke.mjs > results/smoke-chromium.txt
