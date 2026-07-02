#!/usr/bin/env bash
set -euo pipefail

# Ratio-Improvement Evaluation Protocol harness (see ROADMAP.md, "Ratio-Improvement
# Evaluation Protocol"). Runs the tracked non-stream benchmarks with the release binary
# and fails when any corpus compresses to MORE bytes than the tracked baseline, exceeds
# its wall budget, or fails output validation. Reports are written to a temp dir so the
# tracked *_latest.txt files are never clobbered by a check run.
#
# Usage:
#   check_benchmark_budgets.sh                 # all corpora whose assets are present
#   check_benchmark_budgets.sh paper cat       # only the named corpora
#
# The budgets are the 2026-07-02 baseline (balanced profile, release build): bytes are
# the exact tracked compressed sizes, wall budgets are the measured times plus ~10%
# headroom. Re-baseline this table and the ROADMAP table in the same commit as any
# accepted ratio/time trade.

repo_root="$(cd "$(dirname "$0")/../.." && pwd)"
bench_bin="$repo_root/zbit-rs/target/release/zbit-benchmark"
out_dir="$(mktemp -d "${TMPDIR:-/tmp}/zbit-budget-check.XXXXXX")"
trap 'rm -rf "$out_dir"' EXIT

cargo build --release --quiet --manifest-path "$repo_root/zbit-rs/Cargo.toml" --bin zbit-benchmark

# name|input path|max compressed bytes|max compression ms
budgets=(
  "paper|$repo_root/papers/zbit-algorithmsResearch.md|18573|300"
  "primary.3b|$repo_root/assets/primary.3b.bin|562799|4000"
  "cat|$repo_root/assets/cat_challenge.png|2670567|38000"
  "depth_anything|$repo_root/assets/depth_anything_v2_vits.pth|83380762|400000"
)

selected=("$@")
should_run() {
  local name="$1"
  [[ ${#selected[@]} -eq 0 ]] && return 0
  local want
  for want in "${selected[@]}"; do
    [[ "$want" == "$name" ]] && return 0
  done
  return 1
}

fail=0
printf '%-16s %-12s %-12s %-12s %-10s %s\n' corpus bytes max-bytes ms max-ms status
for row in "${budgets[@]}"; do
  IFS='|' read -r name input max_bytes max_ms <<<"$row"
  should_run "$name" || continue
  if [[ ! -f "$input" ]]; then
    printf '%-16s %-12s %-12s %-12s %-10s %s\n' "$name" - "$max_bytes" - "$max_ms" "SKIPPED (asset missing)"
    continue
  fi

  report="$out_dir/$name.txt"
  "$bench_bin" "$input" "$out_dir/$name.zbpk" "$report" >/dev/null

  bytes=$(awk -F': ' '/^Compressed size \(bytes\)/{print $2}' "$report")
  ms=$(awk -F': ' '/^Compression time \(ms\)/{print $2}' "$report")
  validation=$(awk -F': ' '/^Output validation/{print $2}' "$report")
  ms_int=${ms%%.*}

  status="OK"
  if [[ "$validation" != "PASS" ]]; then
    status="FAIL (validation: $validation)"
    fail=1
  elif (( bytes > max_bytes )); then
    status="FAIL (bytes over baseline)"
    fail=1
  elif (( ms_int > max_ms )); then
    status="FAIL (over wall budget)"
    fail=1
  elif (( bytes < max_bytes )); then
    status="OK (bytes improved — re-baseline the budget tables)"
  fi

  printf '%-16s %-12s %-12s %-12s %-10s %s\n' "$name" "$bytes" "$max_bytes" "$ms_int" "$max_ms" "$status"
done

if (( fail )); then
  echo "budget check FAILED" >&2
  exit 1
fi
echo "budget check passed"
