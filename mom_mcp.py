"""P0 MCP probe for verifying the AgentNexus transport and result envelope."""

import argparse
from contextlib import closing
from pathlib import Path
import sqlite3
import time
from typing import Literal

from mcp.server.fastmcp import FastMCP
from pydantic import BaseModel


class ProbeResult(BaseModel):
    ok: bool
    code: Literal["OK", "INVALID_ARGUMENT", "VALUE_OUT_OF_RANGE", "IDEMPOTENCY_CONFLICT"]
    value: int
    revision: int


def create_server(database: Path, response_delay_ms: int = 0) -> FastMCP:
    server = FastMCP("MOM-MCP P0")
    with closing(sqlite3.connect(database)) as db, db:
        db.execute("CREATE TABLE IF NOT EXISTS writes (key TEXT PRIMARY KEY, value INTEGER NOT NULL)")

    def current(db: sqlite3.Connection) -> ProbeResult:
        row = db.execute("SELECT rowid, value FROM writes ORDER BY rowid DESC LIMIT 1").fetchone()
        revision, value = row if row else (0, 0)
        return ProbeResult(ok=True, code="OK", value=value, revision=revision)

    @server.tool()
    def get_probe() -> ProbeResult:
        """Read the current synthetic value and revision."""
        with closing(sqlite3.connect(database)) as db:
            return current(db)

    @server.tool()
    def set_probe(value: int | None = None, idempotency_key: str | None = None) -> ProbeResult:
        """Set a synthetic value from 0 to 100 using a stable idempotency key."""
        with closing(sqlite3.connect(database)) as db, db:
            db.execute("BEGIN IMMEDIATE")
            state = current(db)
            if value is None or not idempotency_key or len(idempotency_key) > 128:
                return state.model_copy(update={"ok": False, "code": "INVALID_ARGUMENT"})
            if not 0 <= value <= 100:
                return state.model_copy(update={"ok": False, "code": "VALUE_OUT_OF_RANGE"})
            previous = db.execute(
                "SELECT rowid, value FROM writes WHERE key = ?", (idempotency_key,)
            ).fetchone()
            if previous:
                revision, recorded_value = previous
                if recorded_value != value:
                    return state.model_copy(update={"ok": False, "code": "IDEMPOTENCY_CONFLICT"})
                return ProbeResult(ok=True, code="OK", value=value, revision=revision)
            cursor = db.execute("INSERT INTO writes VALUES (?, ?)", (idempotency_key, value))
            result = ProbeResult(ok=True, code="OK", value=value, revision=cursor.lastrowid)
        if response_delay_ms:
            time.sleep(response_delay_ms / 1000)
        return result

    return server


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--db", required=True, type=Path, help="Path to the P0 probe database")
    parser.add_argument("--response-delay-ms", type=int, default=0, help="Test-only delay after a new write commits")
    options = parser.parse_args()
    if not 0 <= options.response_delay_ms <= 60_000:
        parser.error("--response-delay-ms must be between 0 and 60000")
    create_server(options.db, options.response_delay_ms).run(transport="stdio")
