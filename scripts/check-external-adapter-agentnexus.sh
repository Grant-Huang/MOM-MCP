#!/usr/bin/env bash
# Check a pinned adapter against a clean AgentNexus snapshot using its existing tests.
set -euo pipefail

if [[ $# -lt 3 || $# -gt 4 || ! ${1:-} =~ ^[0-9a-f]{40}$ || ! ${3:-} =~ ^[0-9a-f]{40}$ ]]; then
    echo "Usage: bash scripts/check-external-adapter-agentnexus.sh ADAPTER_SHA AGENTNEXUS_CHECKOUT HOST_SHA [ADAPTER_GIT_URL]" >&2
    exit 2
fi

adapter=$1
host=$2
host_revision=$3
repository=${4:-}
root=$(git rev-parse --show-toplevel)
if [[ -z "$repository" ]]; then
    repository=$(python3 -c 'from pathlib import Path; import sys; print(Path(sys.argv[1]).as_uri())' "$root")
fi
sandbox=$(mktemp -d)
trap 'rm -rf "$sandbox"' EXIT
git -C "$host" archive "$host_revision" | tar -x -C "$sandbox"

python3 - "$sandbox/crates/server/Cargo.toml" "$repository" "$adapter" <<'PY'
from pathlib import Path
import json
import sys

manifest, repository, revision = sys.argv[1:]
path = Path(manifest)
text = path.read_text()
section = "[dev-dependencies]\n"
if text.count(section) != 1:
    raise SystemExit("Expected one an-server dev-dependencies section")
dependency = f"mom-external-adapter = {{ git = {json.dumps(repository)}, rev = {json.dumps(revision)} }}\n"
path.write_text(text.replace(section, section + dependency, 1))
PY

selectors=(--test mom_world_prod)
for suite in external_order external_http external_observe; do
    git -C "$root" show "$adapter:tests/$suite.rs" |
        sed 's/mom_mcp::/mom_external_adapter::/g' > "$sandbox/crates/server/tests/adapter_$suite.rs"
    selectors+=(--test "adapter_$suite")
done

cd "$sandbox"
cargo update --workspace
cargo test --locked -p an-server "${selectors[@]}"
