#!/usr/bin/env bash
# Verify the adapter can be consumed at an immutable Git revision alongside the host SQLite version.
set -euo pipefail

if [[ $# -lt 1 || $# -gt 2 || ! ${1:-} =~ ^[0-9a-f]{40}$ ]]; then
    echo "Usage: bash scripts/check-external-adapter-consumer.sh FULL_COMMIT_SHA [GIT_URL]" >&2
    exit 2
fi

revision=$1
repository=${2:-}
if [[ -z "$repository" ]]; then
    root=$(git rev-parse --show-toplevel)
    repository=$(python3 -c 'from pathlib import Path; import sys; print(Path(sys.argv[1]).as_uri())' "$root")
fi
sandbox=$(mktemp -d)
trap 'rm -rf "$sandbox"' EXIT
mkdir "$sandbox/src"
python3 - "$sandbox/Cargo.toml" "$repository" "$revision" <<'PY'
from pathlib import Path
import json
import sys

manifest, repository, revision = sys.argv[1:]
Path(manifest).write_text(f"""[package]
name = "external-adapter-consumer-smoke"
version = "0.0.0"
edition = "2024"

[dependencies]
mom-external-adapter = {{ git = {json.dumps(repository)}, rev = {json.dumps(revision)} }}
rusqlite = {{ version = "0.43", features = ["bundled", "fallible_uint"] }}
serde_json = "1"
""")
PY
cat > "$sandbox/src/main.rs" <<'RS'
use mom_external_adapter::{
    external_observe::ExternalObserve,
    external_order::{BoundOrder, ProjectionError, Source, project_order},
};
use rusqlite::Connection;
use serde_json::json;

fn main() {
    ExternalObserve::new([]).expect("adapter registry initializes");
    let binding = BoundOrder {
        source: Source::OpenMes,
        tenant: "sample-tenant",
        site: "sample-site",
        instance: "openmes",
        source_id: "42",
    };
    let payload = |id| json!({"data":{"id":id,"order_no":"WO-42","status":"PENDING"}});
    let order = project_order(&binding, payload(42)).expect("bound work order projects");
    assert_eq!(order.identity.source_id, "42");
    assert_eq!(order.source_status.as_deref(), Some("PENDING"));
    assert_eq!(
        project_order(&binding, payload(43)).unwrap_err(),
        ProjectionError::UnexpectedRecord,
    );

    let connection = Connection::open_in_memory().expect("host SQLite initializes");
    let stored_id: String = connection
        .query_row("SELECT ?1", [order.identity.source_id], |row| row.get(0))
        .unwrap();
    assert_eq!(stored_id, binding.source_id);
}
RS

manifest="$sandbox/Cargo.toml"
tree=$(cargo tree --manifest-path "$manifest" -p mom-external-adapter --prefix none)
if grep -Eq '^(mom-mcp|mom-des|mom-model-[^ ]+|rusqlite|libsqlite3-sys) v' <<<"$tree"; then
    echo "The adapter unexpectedly depends on the simulator or SQLite" >&2
    exit 1
fi
cargo run --locked --quiet --manifest-path "$manifest"
echo "PASS pinned adapter projects a bound order, rejects a mismatched record, and coexists with host SQLite"
