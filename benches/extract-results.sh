#!/usr/bin/env bash
# Extracts a small JSON summary (median/mean point estimates, per batch and
# per logical operation) from criterion's `target/criterion/**/estimates.json`
# output, and writes it to `results/<timestamp>-rust.json` here, so the raw
# numbers are committed (per the issue: "Store raw results under a results
# directory") without committing criterion's much larger HTML/SVG report
# tree.
#
# Usage: benches/extract-results.sh [output-file]
# Run this right after `cargo bench -p concerto-benches`. Requires jq.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
CRITERION_DIR="$REPO_ROOT/target/criterion"
RESULTS_DIR="$SCRIPT_DIR/results"

if [ ! -d "$CRITERION_DIR" ]; then
    echo "error: $CRITERION_DIR not found - run 'cargo bench -p concerto-benches' first." >&2
    exit 1
fi

TIMESTAMP="$(date -u +%Y-%m-%dT%H-%M-%SZ)"
OUT_FILE="${1:-$RESULTS_DIR/${TIMESTAMP}-rust.json}"
mkdir -p "$RESULTS_DIR"

COMMIT="$(git -C "$REPO_ROOT" rev-parse HEAD 2>/dev/null || echo null)"

entries='[]'
while IFS= read -r -d '' estimates_file; do
    rel="${estimates_file#"$CRITERION_DIR"/}"
    # rel looks like: <bench-group>/<...>/<n>/new/estimates.json
    n="$(basename "$(dirname "$(dirname "$estimates_file")")")"
    id="${rel%/*/new/estimates.json}"
    median_ns="$(jq '.median.point_estimate' "$estimates_file")"
    mean_ns="$(jq '.mean.point_estimate' "$estimates_file")"
    entry="$(jq -n --arg id "$id" --argjson n "$n" --argjson median_ns "$median_ns" --argjson mean_ns "$mean_ns" \
        '{id: $id, n: $n, median_ns_per_batch: $median_ns, median_ns_per_op: ($median_ns / $n), mean_ns_per_batch: $mean_ns}')"
    entries="$(jq --argjson e "$entry" '. + [$e]' <<< "$entries")"
done < <(find "$CRITERION_DIR" -name estimates.json -path '*/new/*' -print0 | sort -z)

jq -n \
    --arg harness "cargo bench -p concerto-benches" \
    --arg recorded_at "$(date -u +%Y-%m-%dT%H:%M:%SZ)" \
    --arg commit "$COMMIT" \
    --argjson entries "$entries" \
    '{harness: $harness, recorded_at: $recorded_at, concerto_rust_commit: $commit, entries: $entries}' \
    > "$OUT_FILE"

echo "Wrote $(jq '.entries | length' "$OUT_FILE") entries to ${OUT_FILE#"$REPO_ROOT"/}"
