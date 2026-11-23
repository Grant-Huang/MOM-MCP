import { test, expect } from "@playwright/test";

test("P9B-UI Receive and reconcile a lot", async ({ page }) => {
  await page.goto("/scenarios");
  const button = (name) => page.getByRole("button", { name, exact: true });
  const status = page.getByRole("status");
  await button("加载场景").click();
  await page.getByLabel("已发布场景").selectOption("cnc@24");
  await button("打开场景").click();
  await expect(status).toContainText('"run_id"');
  await button("搜索工具").click();

  async function invoke(name, args, expected) {
    await page.getByLabel("匹配工具").selectOption(name);
    await page.getByLabel("JSON 参数").fill(JSON.stringify(args));
    await button("调用工具").click();
    await expect(status).toContainText(expected);
    return JSON.parse(await status.textContent());
  }

  let cursor = 0;
  async function write(name, args, expected) {
    const result = await invoke(name, { ...args, expected_cursor: cursor, idempotency_key: name }, expected);
    cursor = result.cursor;
    return result;
  }
  const item = { item_id: "WIRE", revision: 1 };
  await write("create_item", { item_id: item.item_id, base_unit: "m" }, '"base_unit": "m"');
  await write("create_item_revision", { ...item, name: "Wire" }, '"status": "draft"');
  await write("activate_revision", { ...item, revision_type: "item" }, '"status": "active"');
  const receipt = { receipt_id: "RC-1" };
  await write("create_receipt", { ...receipt, ...item, quantity: "2.375", requires_inspection: true }, '"quantity": "2.375"');
  await write("assign_receipt_tracking", { ...receipt, lot_id: "LOT-1", supplier_lot: "SUP-42", location: "RAW-02" }, '"status": "tracked"');
  await write("post_receipt", receipt, '"availability": "pending_inspection"');
  const lot = await invoke("get_lot", { lot_id: "LOT-1" }, '"lot": {');
  expect(lot.lot).toMatchObject({ supplier_lot: "SUP-42", location: "RAW-02", quantity: "2.375" });
  const stock = await invoke("get_stock_balance", { ...item, location: "RAW-02" }, '"balance"');
  expect(stock.balance).toMatchObject({ unit: "m", on_hand: "2.375", pending_inspection: "2.375", available: "0" });
});

test("P9B-03 Release inspected inventory", async ({ page }) => {
  await page.goto("/scenarios");
  const button = (name) => page.getByRole("button", { name, exact: true });
  const status = page.getByRole("status");
  await button("加载场景").click();
  await page.getByLabel("已发布场景").selectOption("assembly@24");
  await button("打开场景").click();
  await expect(status).toContainText('"run_id"');
  const { run_id } = JSON.parse(await status.textContent());

  async function invoke(name, arguments_) {
    const response = await page.request.post("/api/scenarios", { data: { name: "call_tool", arguments: { run_id, name, arguments: arguments_ } } });
    expect(response.ok()).toBeTruthy();
    return response.json();
  }

  let cursor = 0;
  async function write(name, args) {
    const result = await invoke(name, { ...args, expected_cursor: cursor, idempotency_key: name });
    expect(result.ok, JSON.stringify(result)).toBe(true);
    cursor = result.cursor;
    return result;
  }

  const item = { item_id: "WIRE", revision: 1 };
  await write("create_item", { item_id: item.item_id, base_unit: "m" });
  await write("create_item_revision", { ...item, name: "Wire" });
  await write("activate_revision", { ...item, revision_type: "item" });
  const receipt = { receipt_id: "RC-1" };
  await write("create_receipt", { ...receipt, ...item, quantity: "2", requires_inspection: true });
  await write("assign_receipt_tracking", { ...receipt, lot_id: "LOT-1", supplier_lot: "SUP-42", location: "RAW-02" });
  await write("post_receipt", receipt);
  expect((await invoke("get_material_availability", item)).balance.available).toBe("0");
  const plan = { plan_id: "QA", revision: 1 };
  await write("create_inspection_plan_revision", { ...plan, characteristics: { width: { unit: "mm", lower_limit: 1, upper_limit: 2 } } });
  await write("activate_inspection_plan_revision", plan);
  const inspection = { inspection_id: "QC-1" };
  await write("create_inspection", { ...receipt, ...inspection, work_order: "WO-E", specification_version: "QA@1" });
  await write("record_measurement", { ...inspection, characteristic: "width", unit: "mm", value: 2, lower_limit: 1, upper_limit: 2 });
  await write("complete_inspection", inspection);

  await button("搜索工具").click();
  await page.getByLabel("匹配工具").selectOption("release_receipt_inspection");
  await page.getByLabel("JSON 参数").fill(JSON.stringify({ ...receipt, ...inspection, expected_cursor: cursor, idempotency_key: "release" }));
  await button("调用工具").click();
  await expect(status).toContainText('"released_inspection_id": "QC-1"');
  expect((await invoke("get_material_availability", item)).balance.available).toBe("2");
});
