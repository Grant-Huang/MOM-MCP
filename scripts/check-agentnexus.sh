#!/usr/bin/env bash
set -euo pipefail

project=$(cd "$(dirname "$0")/.." && pwd)
agentnexus=$(realpath "${1:-../multi-agent-conv-mvp}")
revision=e0a21d32e5597ab785cf717144590711584d519a
scratch=$(mktemp -d)
trap 'rm -rf "$scratch"' EXIT

cargo build --locked --manifest-path "$project/Cargo.toml" --target-dir "$project/target"
git -C "$agentnexus" archive "$revision" | tar -x -C "$scratch"
cp "$project/tests/agentnexus/probe.rs" "$scratch/crates/agents/tests/mom_probe.rs"
cp "$project/tests/agentnexus/industries.rs" "$scratch/crates/agents/tests/mom_industries.rs"
cat "$project/tests/agentnexus/des_baseline.rs" >> "$scratch/crates/des-mcp/tests/stdio.rs"
export CARGO_TARGET_DIR="${MOM_AGENT_TARGET_DIR:-$project/target/agentnexus}"
mkdir -p "$project/test-results"
export MOM_DES_REPORT="$project/test-results/des-baseline.json"
rm -f "$MOM_DES_REPORT"
export MOM_PROBE_BINARY="$project/target/debug/mom-mcp"
export MOM_PROBE_DATABASE="$scratch/probe.sqlite3"
(
  cd "$scratch"
  cargo test --locked -p an-agents --test mom_probe
  cargo test --locked -p an-des -p an-des-mcp
  if [[ ${MOM_ACCEPTANCE_PHASE:-P0} == P10 || ${MOM_ACCEPTANCE_PHASE:-P0} == P11 || ${MOM_ACCEPTANCE_PHASE:-P0} == P12 || ${MOM_ACCEPTANCE_PHASE:-P0} == P13 || ${MOM_ACCEPTANCE_PHASE:-P0} == P14 || ${MOM_ACCEPTANCE_PHASE:-P0} == P15 || ${MOM_ACCEPTANCE_PHASE:-P0} > P5 ]]; then
    export MOM_INDUSTRY_REPORT="$project/test-results/agentnexus-industries.json"
    export MOM_RECOVERY_REPORT="$project/test-results/agentnexus-recovery.json"
    rm -f "$MOM_RECOVERY_REPORT"
    rm -f "$MOM_INDUSTRY_REPORT"
    if [[ ${MOM_ACCEPTANCE_PHASE:-P0} == P9D || ${MOM_ACCEPTANCE_PHASE:-P0} == P10 || ${MOM_ACCEPTANCE_PHASE:-P0} == P11 || ${MOM_ACCEPTANCE_PHASE:-P0} == P12 || ${MOM_ACCEPTANCE_PHASE:-P0} == P13 || ${MOM_ACCEPTANCE_PHASE:-P0} == P14 || ${MOM_ACCEPTANCE_PHASE:-P0} == P15 ]]; then
      export MOM_P9D_CNC_INPUT="$project/test-results/p9d-cnc-closure.json"
      export MOM_P9D_AGENT_REPORT="$project/test-results/p9d-agentnexus-cnc-closure.json"
      if [[ ${MOM_ACCEPTANCE_PHASE:-P0} == P15 ]]; then
        export MOM_P15_SESSION_REPORT="$project/test-results/p15-five-industry-session.json"
      fi
      test -s "$MOM_P9D_CNC_INPUT"
      cargo test --locked -p an-agents --test mom_industries
      test -s "$MOM_P9D_AGENT_REPORT"
      if [[ ${MOM_ACCEPTANCE_PHASE:-P0} == P15 ]]; then test -s "$MOM_P15_SESSION_REPORT"; fi
    else
      cargo test --locked -p an-agents --test mom_industries -- --skip p9d_agentnexus_cnc_closure
    fi
  fi
)
