// Executable examples for the candidate ontology. Inputs start after transport
// classification; Carbon rows are synthetic until its MCP envelope is verified.
import assert from "node:assert/strict";
import test from "node:test";

const observations = [
  {
    vendor: "odoo",
    instance: "odoo-sample",
    payload: [{ id: 42, name: "WO-42", state: "confirmed", write_date: "2026-11-22 16:21:58" }],
    status: "confirmed",
  },
  {
    vendor: "openmes",
    instance: "openmes-sample",
    payload: { data: { id: 42, order_no: "WO-42", status: "ACCEPTED", updated_at: "2026-11-22T16:20:00.000000Z" } },
    status: "ACCEPTED",
    sourceUpdatedAt: "2026-11-22T16:20:00.000Z",
  },
  {
    vendor: "carbon",
    instance: "carbon-sample",
    payload: { id: "42", jobId: "WO-42", status: "Ready" },
    status: "Ready",
  },
];

function readRecord(vendor, payload) {
  switch (vendor) {
    case "odoo": {
      assert.ok(Array.isArray(payload) && payload.length === 1);
      const { id, name, state } = payload[0];
      return { id, displayNumber: name, status: state };
    }
    case "openmes": {
      const { id, order_no, status, updated_at } = payload.data;
      const sourceUpdatedAt = typeof updated_at === "string" && updated_at.endsWith("Z")
        && Number.isFinite(Date.parse(updated_at)) ? new Date(updated_at).toISOString() : undefined;
      return { id, displayNumber: order_no, status, sourceUpdatedAt };
    }
    case "carbon": {
      const { id, jobId, status } = payload;
      return { id, displayNumber: jobId, status };
    }
  }
  throw new Error("Unsupported source");
}

function project(binding, payload) {
  const { id, displayNumber, status, sourceUpdatedAt } = readRecord(binding.vendor, payload);
  assert.ok(typeof id === "string" ? id.length > 0 : Number.isSafeInteger(id) && id > 0);
  assert.equal(String(id), binding.requestedId);
  assert.ok(typeof displayNumber === "string" && displayNumber.trim());
  assert.ok(status == null || typeof status === "string");
  return {
    identity: [binding.tenant, binding.site, binding.instance, "work_order", String(id)],
    displayNumber,
    sourceStatus: status ?? null,
    normalizedStatus: null,
    mappingIssue: status == null ? "missing" : "unmapped",
    mappingVersion: "0.1",
    ...(sourceUpdatedAt === undefined ? {} : { sourceUpdatedAt }),
  };
}

function binding(sample, overrides = {}) {
  const { vendor, instance } = sample;
  return { vendor, instance, tenant: "test-tenant", site: "plant-a", requestedId: "42", ...overrides };
}

for (const sample of observations) {
  test(`${sample.vendor}: preserve the source state and verified timestamp`, () => {
    const observed = project(binding(sample), sample.payload);
    assert.deepEqual(observed, {
      identity: ["test-tenant", "plant-a", sample.instance, "work_order", "42"],
      displayNumber: "WO-42",
      sourceStatus: sample.status,
      normalizedStatus: null,
      mappingIssue: "unmapped",
      mappingVersion: "0.1",
      ...(sample.sourceUpdatedAt ? { sourceUpdatedAt: sample.sourceUpdatedAt } : {}),
    });
  });
}

test("same record ID and display number remain distinct across source, site and tenant", () => {
  const [sample] = observations;
  const results = [
    ...observations.map((item) => project(binding(item), item.payload)),
    project(binding(sample, { site: "plant-b" }), sample.payload),
    project(binding(sample, { tenant: "other-tenant" }), sample.payload),
  ];
  assert.equal(new Set(results.map(({ identity }) => JSON.stringify(identity))).size, results.length);
});

test("reject a mismatched or lossy source ID", () => {
  const [sample] = observations;
  assert.throws(() => project(binding(sample, { requestedId: "43" }), sample.payload));
  const lossyId = Number.MAX_SAFE_INTEGER + 1;
  assert.throws(() => project(binding(sample, { requestedId: String(lossyId) }),
    [{ ...sample.payload[0], id: lossyId }]));
});

test("report a missing state separately and reject a malformed state", () => {
  const sample = observations[2];
  const observed = project(binding(sample), { ...sample.payload, status: null });
  assert.equal(observed.mappingIssue, "missing");
  assert.equal(observed.normalizedStatus, null);
  assert.throws(() => project(binding(sample), { ...sample.payload, status: {} }));
});

test("reject multiple matches in a single-object query", () => {
  const [sample] = observations;
  assert.throws(() => project(binding(sample), [...sample.payload, ...sample.payload]));
});
