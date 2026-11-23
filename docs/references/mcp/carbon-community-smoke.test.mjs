import assert from "node:assert/strict";
import test from "node:test";
import { checkCommunityIsolation } from "./carbon-community-smoke.mjs";

function fixture(change = (rows) => Response.json(rows)) {
  const calls = [];
  return {
    calls,
    origin: "http://127.0.0.1:54321",
    projectKey: "fixture-project-key",
    a: { token: "fixture-user-a", job: "fixture-id-a" },
    b: { token: "fixture-user-b", job: "fixture-id-b" },
    async request(url, init) {
      const user = init.headers.Authorization.at(-1);
      const record = url.searchParams.get("id").at(-1);
      calls.push([user, record]);
      assert.equal(url.pathname, "/rest/v1/jobs");
      assert.deepEqual([...url.searchParams.keys()], ["select", "id", "limit"]);
      assert.equal(url.searchParams.get("limit"), "2");
      assert.equal(init.method, "GET");
      assert.equal(init.redirect, "error");
      assert.equal(init.headers.apikey, "fixture-project-key");
      const rows = user === record ? [{ id: `fixture-id-${record}`, companyId: `company-${record}`,
        jobId: "same-display-number", status: "Ready" }] : [];
      return change(rows, user, record);
    },
  };
}

test("proves both jobs exist and checks both cross-company reads with no company filter", async () => {
  const config = fixture();
  const report = await checkCommunityIsolation(config);
  assert.deepEqual(config.calls, [["a", "a"], ["b", "b"], ["a", "b"], ["b", "a"]]);
  assert.deepEqual(report, { ownCompany: { a: "record", b: "record" }, otherCompany: { a: "empty", b: "empty" } });
  for (const field of ["fixture", "company-a", "same-display-number", "Ready"]) {
    assert.equal(JSON.stringify(report).includes(field), false);
  }
});

test("does not accept absence as isolation if the second owner cannot see its record", async () => {
  const config = fixture((rows, user) => Response.json(user === "b" ? [] : rows));
  await assert.rejects(checkCommunityIsolation(config), /two distinct visible jobs/);
  assert.equal(config.calls.length, 2);
});

test("rejects a cross-company leak even if the opposite direction is isolated", async () => {
  const config = fixture((rows, user, record) =>
    Response.json(user === "a" && record === "b" ? [{ id: "fixture-id-b" }] : rows));
  await assert.rejects(checkCommunityIsolation(config), /cross-company job was visible/);
});

test("distinguishes an expired credential from an empty cross-company result", async () => {
  const config = fixture((rows, user, record) =>
    user === record ? Response.json(rows) : new Response(null, { status: 401 }));
  await assert.rejects(checkCommunityIsolation(config), /cross-company read returned HTTP 401/);
});

test("rejects unsafe endpoints and invalid filters before any request", async () => {
  const config = fixture();
  await assert.rejects(checkCommunityIsolation({ ...config, origin: "http://example.test" }), /Invalid Supabase origin/);
  await assert.rejects(checkCommunityIsolation({ ...config, b: { ...config.b, job: "bad,id" } }), /distinct company sessions/);
  assert.equal(config.calls.length, 0);
});
