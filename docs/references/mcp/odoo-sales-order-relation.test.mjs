import assert from "node:assert/strict";
import test from "node:test";

const evidencePath = "mrp.production.sale_line_id → sale.order.line.order_id";

function isId(value) {
  return Number.isSafeInteger(value) && value > 0;
}

function relationId(value) {
  if (value === false) return null;
  if (Array.isArray(value) && value.length === 2 && isId(value[0])) return value[0];
  throw new TypeError("Invalid Odoo relation");
}

// Each input is the result of one exact-ID query with the same authorized company and reader.
function project(scope, workOrderId, { workOrders, salesLines, salesOrders }) {
  const read = (rows, id) => {
    if (!Array.isArray(rows)) throw new TypeError("Invalid Odoo response");
    if (!rows.length) return { outcome: "unresolved" };
    if (rows.length !== 1) return { outcome: "conflict" };
    if (!isId(rows[0]?.id)) throw new TypeError("Invalid Odoo record ID");
    if (rows[0].id !== id) return { outcome: "conflict" };
    if (relationId(rows[0].company_id) !== scope.companyId) return { outcome: "unresolved" };
    return { row: rows[0] };
  };
  const identity = (objectType, id) => ({ ...scope, objectType, sourceId: String(id) });

  try {
    if (!isId(workOrderId) || !isId(scope.companyId)) throw new TypeError("Invalid binding");
    const workOrder = read(workOrders, workOrderId);
    if (!workOrder.row) return workOrder;
    const lineId = relationId(workOrder.row.sale_line_id);
    if (lineId === null) return { outcome: "no_direct_relation" };
    const salesLine = read(salesLines, lineId);
    if (!salesLine.row) return salesLine;
    const orderId = relationId(salesLine.row.order_id);
    if (orderId === null) return { outcome: "unresolved" };
    const salesOrder = read(salesOrders, orderId);
    if (!salesOrder.row) return salesOrder;
    return {
      outcome: "linked",
      relation: "fulfills_sales_order",
      from: identity("work_order", workOrderId),
      to: identity("sales_order", orderId),
      evidence: { path: evidencePath, salesLineId: String(lineId) },
    };
  } catch (error) {
    if (error instanceof TypeError) return { outcome: "invalid_source_response" };
    throw error;
  }
}

const scope = { tenant: "tenant-a", site: "site-a", instance: "odoo-a", companyId: 7 };
const company = [7, "Company A"];

function fixture() {
  return {
    workOrders: [{ id: 42, company_id: company, name: "MO-42", origin: "SO-duplicate",
      sale_line_id: [17, "Sales line"] }],
    salesLines: [{ id: 17, company_id: company, order_id: [81, "SO-actual"] }],
    salesOrders: [{ id: 81, company_id: company, name: "SO-actual" }],
  };
}

test("links a manufacturing order to the sales order identified by its origin line", () => {
  assert.deepEqual(project(scope, 42, fixture()), {
    outcome: "linked", relation: "fulfills_sales_order",
    from: { ...scope, objectType: "work_order", sourceId: "42" },
    to: { ...scope, objectType: "sales_order", sourceId: "81" },
    evidence: { path: evidencePath, salesLineId: "17" },
  });
});

test("preserves distinct identities for the same record IDs in different instances", () => {
  const a = project(scope, 42, fixture());
  const b = project({ ...scope, instance: "odoo-b" }, 42, fixture());
  assert.notDeepEqual(a.from, b.from);
  assert.notDeepEqual(a.to, b.to);
});

test("ignores matching document names when the direct relation is absent", () => {
  const snapshot = fixture();
  snapshot.workOrders[0].sale_line_id = false;
  snapshot.salesOrders[0].name = snapshot.workOrders[0].origin;
  assert.deepEqual(project(scope, 42, snapshot), { outcome: "no_direct_relation" });
});

test("keeps every invisible or cross-company hop unresolved without exposing an ID", () => {
  for (const model of ["workOrders", "salesLines", "salesOrders"]) {
    const missing = fixture();
    missing[model] = [];
    assert.deepEqual(project(scope, 42, missing), { outcome: "unresolved" });
    const crossCompany = fixture();
    crossCompany[model][0].company_id = [8, "Company B"];
    assert.deepEqual(project(scope, 42, crossCompany), { outcome: "unresolved" });
  }
});

test("rejects an exact-ID query with a different record or conflicting sales-line rows", () => {
  const mismatched = fixture();
  mismatched.salesOrders[0].id = 82;
  assert.deepEqual(project(scope, 42, mismatched), { outcome: "conflict" });
  const conflicting = fixture();
  conflicting.salesLines.push({ ...conflicting.salesLines[0], order_id: [82, "SO-other"] });
  assert.deepEqual(project(scope, 42, conflicting), { outcome: "conflict" });
});

test("rejects malformed relations and lossy source IDs", () => {
  const malformed = fixture();
  malformed.workOrders[0].sale_line_id = [Number.MAX_SAFE_INTEGER + 1, "Sales line"];
  assert.deepEqual(project(scope, 42, malformed), { outcome: "invalid_source_response" });
  malformed.workOrders[0].sale_line_id = "17";
  assert.deepEqual(project(scope, 42, malformed), { outcome: "invalid_source_response" });
});

test("distinguishes malformed responses from an invisible or conflicting record", () => {
  for (const rows of [{}, [null], [{ id: "81" }], [{ id: Number.MAX_SAFE_INTEGER + 1 }]]) {
    const snapshot = { ...fixture(), salesOrders: rows };
    assert.deepEqual(project(scope, 42, snapshot), { outcome: "invalid_source_response" });
  }
});
