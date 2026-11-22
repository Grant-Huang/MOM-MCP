import assert from "node:assert/strict";
import test from "node:test";
import { checkCarbon } from "./carbon-mcp-smoke.mjs";

function options({ refusal = true, httpStatus, rowId = "own" } = {}) {
  const calls = [];
  async function request(_, init) {
    const { id, method, params } = JSON.parse(init.body);
    calls.push([method, params]);
    assert.equal(init.redirect, "error");
    if (httpStatus) return new Response(null, { status: httpStatus });
    if (method === "notifications/initialized") return new Response(null, { status: 202 });
    let result;
    if (method === "initialize") {
      result = { protocolVersion: "2025-11-25" };
    } else if (params.name === "describe_tool") {
      result = { content: [{ type: "text", text: "production_getJob" }] };
    } else if (params.arguments.arguments.id === "own") {
      result = { content: [{ type: "text", text: JSON.stringify({ id: rowId, jobId: "WO-1", status: "Draft" }) }] };
    } else {
      result = { content: [{ type: "text", text: "fixture refusal" }], isError: refusal };
    }
    return Response.json({ jsonrpc: "2.0", id, result });
  }
  return { url: "https://example.test/api/mcp", token: "fixture-token",
    visibleId: "own", hiddenId: "other", calls, request };
}

test("captures sanitized success and a tool refusal requiring classification", async () => {
  const config = options();
  const result = await checkCarbon(config);
  assert.equal(result.visible.outcome, "record");
  assert.equal(result.otherCompany.classification, "requires-review");
  assert.equal(JSON.stringify(result).includes("WO-1"), false);
  assert.deepEqual(config.calls.slice(-2).map(([, params]) => params.arguments), [
    { name: "production_getJob", arguments: { id: "own" }, fields: ["id", "jobId", "status", "updatedAt"] },
    { name: "production_getJob", arguments: { id: "other" }, fields: ["id", "jobId", "status", "updatedAt"] },
  ]);
});

test("reports entitlement denial before attempting the read", async () => {
  const config = options({ httpStatus: 402 });
  await assert.rejects(checkCarbon(config), /initialize returned HTTP 402/);
  assert.equal(config.calls.length, 1);
});

test("rejects an unexpected success for another company's fixture", async () => {
  await assert.rejects(checkCarbon(options({ refusal: false })), /tool-level refusal/);
});

test("rejects a mismatched visible record before reading the other fixture", async () => {
  const config = options({ rowId: "wrong" });
  await assert.rejects(checkCarbon(config), /candidate contract/);
  assert.equal(config.calls.length, 4);
});
