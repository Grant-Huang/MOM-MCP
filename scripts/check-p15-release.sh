#!/usr/bin/env bash
set -euo pipefail

project=$(cd "$(dirname "$0")/.." && pwd)
cd "$project"
: "${MOM_AGENTNEXUS:?Set MOM_AGENTNEXUS to the read-only AgentNexus checkout}"
candidate=$(git rev-parse --verify "${1:?Supply the candidate Git commit}^{commit}")
scratch=$(mktemp -d)
trap 'rm -rf "$scratch"' EXIT
mkdir -p "$scratch/source" "$project/test-results"
git archive "$candidate" | tar -x -C "$scratch/source"
cargo build --release --locked --manifest-path "$scratch/source/Cargo.toml" \
  --target-dir "${MOM_RELEASE_TARGET_DIR:-$project/target}"

output=$(mktemp -d "$project/test-results/p15-release-${candidate:0:12}.XXXXXX")
install -m 755 "${MOM_RELEASE_TARGET_DIR:-$project/target}/release/mom-mcp" "$output/mom-mcp"
export MOM_ACCEPTANCE_BINARY="$output/mom-mcp"
npm run acceptance -- P15 --gate 2>&1 | tee "$output/acceptance.log"

for evidence in browser.json five-industry-catalog.json industry-replay.json restart-recovery.json \
  agentnexus-industries.json agentnexus-recovery.json des-baseline.json \
  p9d-cnc-closure.json p9d-assembly-closure.json p9d-smt-lot-holds.json \
  p9d-agentnexus-cnc-closure.json p15-five-industry-session.json p15-concurrency-compatibility.json; do
  test -s "$project/test-results/$evidence"
  cp "$project/test-results/$evidence" "$output/"
done
{
  printf 'candidate_commit=%s\n' "$candidate"
  printf 'acceptance_base_commit=%s\n' "$(git rev-parse HEAD)"
  printf 'gate=PASS\n'
  rustc --version
  cargo --version
  node --version
} >"$output/manifest.txt"
(
  cd "$output"
  sha256sum mom-mcp acceptance.log manifest.txt ./*.json > SHA256SUMS
  sha256sum --check SHA256SUMS
)
tar -czf "$output.tar.gz" -C "$output" .
printf 'P15 evidence: %s.tar.gz\n' "$output"
