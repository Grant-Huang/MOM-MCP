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
