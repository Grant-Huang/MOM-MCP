import asyncio
from contextlib import asynccontextmanager
from pathlib import Path
import sys
from tempfile import TemporaryDirectory
import unittest

from mcp import ClientSession, StdioServerParameters
from mcp.client.stdio import stdio_client

SERVER = Path(__file__).resolve().parents[1] / "mom_mcp.py"


@asynccontextmanager
async def connect(db, delay_ms=0):
    args = [str(SERVER), "--db", str(db), "--response-delay-ms", str(delay_ms)]
    async with stdio_client(StdioServerParameters(command=sys.executable, args=args)) as streams:
        async with ClientSession(*streams) as client:
            await client.initialize()
            yield client


async def call(client, name, args=None):
    result = await client.call_tool(name, args)
    assert not result.isError, result.content
    assert isinstance(result.structuredContent, dict), result
    return result.structuredContent


class StdioTest(unittest.IsolatedAsyncioTestCase):
    async def test_discovery_writes_and_business_errors(self):
        with TemporaryDirectory() as directory:
            async with connect(Path(directory) / "probe.sqlite3") as client:
                tools = await client.list_tools()
                self.assertEqual({tool.name for tool in tools.tools}, {"get_probe", "set_probe"})
                self.assertTrue(all(tool.outputSchema for tool in tools.tools))
                self.assertEqual(await call(client, "get_probe"), dict(ok=True, code="OK", value=0, revision=0))
                args = dict(value=42, idempotency_key="write-1")
                expected = dict(ok=True, code="OK", value=42, revision=1)
                self.assertEqual(await call(client, "set_probe", args), expected)
                self.assertEqual(await call(client, "set_probe", args), expected)
                for invalid, code in [
                    ({}, "INVALID_ARGUMENT"),
                    (dict(args, value=101), "VALUE_OUT_OF_RANGE"),
                    (dict(args, value=43), "IDEMPOTENCY_CONFLICT"),
                ]:
                    self.assertEqual(await call(client, "set_probe", invalid), dict(expected, ok=False, code=code))
                self.assertEqual(await call(client, "get_probe"), expected)

    async def test_timeout_then_restart_and_replay(self):
        with TemporaryDirectory() as directory:
            db = Path(directory) / "probe.sqlite3"
            args = dict(value=21, idempotency_key="lost-response")
            async with connect(db, delay_ms=5000) as client:
                with self.assertRaises(TimeoutError):
                    await asyncio.wait_for(call(client, "set_probe", args), timeout=1)
            async with connect(db) as client:
                expected = dict(ok=True, code="OK", value=21, revision=1)
                self.assertEqual(await call(client, "get_probe"), expected)
                self.assertEqual(await call(client, "set_probe", args), expected)
                self.assertEqual((await call(client, "set_probe", dict(value=22, idempotency_key="next")))["revision"], 2)
