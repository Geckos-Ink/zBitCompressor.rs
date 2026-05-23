#!/usr/bin/env bash
set -euo pipefail

# Benchmarks zBit normal (.zbpk) compression on the Depth Anything V2 (vits) PyTorch model
# file. The asset is a PyTorch checkpoint (zipped pickle archive containing learned float
# tensors) and is a useful corpus addition because its data profile differs from a PNG: the
# bulk is dense quasi-random float32 weights with some structured headers, layer names, and
# tensor shape metadata interleaved by the PyTorch serializer.
#
# Mirrors the surface area of benchmark_cat_challenge.sh: downloads the asset on first run,
# prints a short asset summary, then invokes the standard `zbit-benchmark` binary to produce
# a single tracked report. Stream compression is intentionally not exercised here — we keep
# this script narrow to the classic-test shape (paper / primary / cat-normal).

repo_root="$(cd "$(dirname "$0")/../.." && pwd)"
asset_dir="$repo_root/assets"
asset_path="$asset_dir/depth_anything_v2_vits.pth"
asset_url="https://geckos.ink/zbit/depth_anything_v2_vits.pth"

pack_path="$repo_root/zbit-rs/benchmark_depth_anything.zbpk"
report_path="$repo_root/zbit-rs/benchmark_depth_anything_latest.txt"

mkdir -p "$asset_dir"

if [[ ! -f "$asset_path" ]]; then
  echo "Downloading depth_anything asset to $asset_path"
  if command -v curl >/dev/null 2>&1; then
    curl -fL --retry 5 --retry-delay 2 "$asset_url" -o "$asset_path"
  elif command -v wget >/dev/null 2>&1; then
    wget -O "$asset_path" "$asset_url"
  else
    echo "Neither curl nor wget is available." >&2
    exit 1
  fi
else
  echo "Using existing asset at $asset_path"
fi

python3 - "$asset_path" <<'PY'
import pathlib
import sys

path = pathlib.Path(sys.argv[1])
data = path.read_bytes()
size_mb = len(data) / (1024 * 1024)

msg = [f"asset-bytes={len(data)} ({size_mb:.2f} MiB)"]
msg.append(f"header8={data[:8].hex() if len(data) >= 8 else 'short'}")

# .pth files are typically ZIP-archived pickle streams. Surface a tiny hint about whether
# the magic matches so a corrupted download is obvious here rather than inside the encoder.
if len(data) >= 4 and data[:4] == b"PK\x03\x04":
    msg.append("magic=PK-ZIP")
elif len(data) >= 2 and data[:2] in (b"\x1f\x8b", b"PK"):
    msg.append("magic=archive-ish")
else:
    msg.append("magic=unknown")

print(" | ".join(msg))
PY

cargo run --release --manifest-path "$repo_root/zbit-rs/Cargo.toml" --bin zbit-benchmark -- \
  "$asset_path" \
  "$pack_path" \
  "$report_path"

rm -f "$pack_path"

echo "Benchmark report updated: $report_path"
