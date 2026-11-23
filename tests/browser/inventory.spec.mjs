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

test("P9C-UI Reserve and pick an eligible lot", async ({ page }) => {
  await page.goto("/scenarios");
  const button = (name) => page.getByRole("button", { name, exact: true });
  const status = page.getByRole("status");
  await button("加载场景").click();
  await page.getByLabel("已发布场景").selectOption("assembly@25");
  await button("打开场景").click();
  await expect(status).toContainText('"run_id"');
  const { run_id } = JSON.parse(await status.textContent());

  async function invoke(name, arguments_) {
    const response = await page.request.post("/api/scenarios", {
      data: { name: "call_tool", arguments: { run_id, name, arguments: arguments_ } },
    });
    expect(response.ok()).toBeTruthy();
    return response.json();
  }
  let cursor = 0;
  async function write(name, args) {
    const result = await invoke(name, { ...args, expected_cursor: cursor, idempotency_key: name + cursor });
    expect(result.ok, JSON.stringify(result)).toBe(true);
    cursor = result.cursor;
    return result;
  }

  for (const [item_id, base_unit] of [["FRAME", "each"], ["WIRE", "m"]]) {
    await write("create_item", { item_id, base_unit });
    await write("create_item_revision", { item_id, revision: 1, name: item_id });
    await write("activate_revision", { item_id, revision: 1, revision_type: "item" });
  }
  const frame = { item_id: "FRAME", revision: 1 };
  await write("create_bom_revision", frame);
  await write("set_bom_components", { ...frame, components: { WIRE: { revision: 1, quantity: 2, unit: "m" } } });
  await write("activate_revision", { ...frame, revision_type: "bom" });
  await write("create_route_revision", frame);
  await write("set_route_operations", { ...frame, operations: { build: { work_center: "CELL-A", predecessors: [] } } });
  await write("activate_revision", { ...frame, revision_type: "route" });
  const plan = { plan_id: "QA", revision: 1 };
  await write("create_inspection_plan_revision", { ...plan, characteristics: { width: { unit: "mm", lower_limit: 1, upper_limit: 2 } } });
  await write("activate_inspection_plan_revision", plan);
  const order = { work_order: "WO-E", definition_selection: { item_id: "FRAME", inspection_plan_id: "QA" } };
  const ready = await invoke("get_release_readiness", order);
  await write("release_work_order", { ...order, snapshot_fingerprint: ready.definition_snapshot.fingerprint });
  const receipt = { receipt_id: "RC-1" };
  const lot = { lot_id: "LOT-1" };
  await write("create_receipt", { ...receipt, item_id: "WIRE", revision: 1, quantity: "2", requires_inspection: false });
  await write("assign_receipt_tracking", { ...receipt, ...lot, supplier_lot: "SUP-42", location: "RAW-02" });
  await write("post_receipt", receipt);
  const reservation = { reservation_id: "RS-1" };
  await write("reserve_material", { ...reservation, ...lot, work_order: order.work_order, quantity: "1.5" });
  await write("pick_material", reservation);
  await button("搜索工具").click();
  await page.getByLabel("匹配工具").selectOption("list_reservations");
  await page.getByLabel("JSON 参数").fill(JSON.stringify(lot));
  await button("调用工具").click();
  await expect(status).toContainText('"status": "picked"');
  const found = JSON.parse(await status.textContent());
  expect(found.reservations[0]).toMatchObject({ reservation_id: "RS-1", work_order: "WO-E", location: "RAW-02" });
  await write("unpick_material", reservation);
  await write("release_reservation", reservation);
  const stock = await invoke("get_material_availability", { item_id: "WIRE", revision: 1 });
  expect(stock.balance).toMatchObject({ on_hand: "2", reserved: "0", available: "2" });
});

for (const { title, scenario, work_order, operation, work_center, report, returns, transfer } of [
  { title: "P9C-03 Issue picked stock to an operation", scenario: "assembly@26", work_order: "WO-E", operation: "build", work_center: "CELL-A" },
  { title: "P9C-05 Record issued material in a partial report", scenario: "cnc@28", work_order: "WO-1", operation: "machining", work_center: "MACH-01", report: true },
  { title: "P9C-07 Return unused issued material", scenario: "cnc@29", work_order: "WO-1", operation: "machining", work_center: "MACH-01", report: true, returns: true },
  { title: "P9C-09 Post a stock transfer", scenario: "cnc@30", work_order: "WO-1", operation: "machining", work_center: "MACH-01", transfer: true },
]) {
  test(title, async ({ page }) => {
    await page.goto("/scenarios");
    const button = (name) => page.getByRole("button", { name, exact: true });
    const status = page.getByRole("status");
    await button("加载场景").click();
    await page.getByLabel("已发布场景").selectOption(scenario);
    await button("打开场景").click();
    await expect(status).toContainText('"run_id"');
    const { run_id } = JSON.parse(await status.textContent());

    async function invoke(name, arguments_) {
      const response = await page.request.post("/api/scenarios", {
        data: { name: "call_tool", arguments: { run_id, name, arguments: arguments_ } },
      });
      expect(response.ok()).toBeTruthy();
      return response.json();
    }
    let cursor = 0;
    async function write(name, args) {
      const result = await invoke(name, { ...args, expected_cursor: cursor, idempotency_key: name + cursor });
      expect(result.ok, JSON.stringify(result)).toBe(true);
      cursor = result.cursor;
      return result;
    }

    for (const [item_id, base_unit] of [["FRAME", "each"], ["WIRE", "m"]]) {
      await write("create_item", { item_id, base_unit });
      await write("create_item_revision", { item_id, revision: 1, name: item_id });
      await write("activate_revision", { item_id, revision: 1, revision_type: "item" });
    }
    const frame = { item_id: "FRAME", revision: 1 };
    await write("create_bom_revision", frame);
    await write("set_bom_components", { ...frame, components: { WIRE: { revision: 1, quantity: 2, unit: "m" } } });
    await write("activate_revision", { ...frame, revision_type: "bom" });
    await write("create_route_revision", frame);
    await write("set_route_operations", { ...frame, operations: { [operation]: { work_center, predecessors: [] } } });
    await write("activate_revision", { ...frame, revision_type: "route" });
    const plan = { plan_id: "QA", revision: 1 };
    await write("create_inspection_plan_revision", { ...plan, characteristics: { width: { unit: "mm", lower_limit: 1, upper_limit: 2 } } });
    await write("activate_inspection_plan_revision", plan);
    const order = { work_order, definition_selection: { item_id: "FRAME", inspection_plan_id: "QA" } };
    const ready = await invoke("get_release_readiness", order);
    await write("release_work_order", { ...order, snapshot_fingerprint: ready.definition_snapshot.fingerprint });
    const receipt = { receipt_id: "RC-1" };
    const lot = { lot_id: "LOT-1" };
    await write("create_receipt", { ...receipt, item_id: "WIRE", revision: 1, quantity: "2", requires_inspection: false });
    await write("assign_receipt_tracking", { ...receipt, ...lot, supplier_lot: "SUP-42", location: "RAW-02" });
    await write("post_receipt", receipt);
    if (transfer) {
      const draft = await write("create_stock_transfer", {
        ...lot, transfer_id: "TR-1", source_location: "RAW-02", destination_location: "RAW-01", reason: "restock",
      });
      expect(draft.transfer.status).toBe("draft");
      await button("搜索工具").click();
      await page.getByLabel("匹配工具").selectOption("post_stock_transfer");
      await page.getByLabel("JSON 参数").fill(JSON.stringify({ transfer_id: "TR-1", expected_cursor: cursor, idempotency_key: "transfer" }));
      await button("调用工具").click();
      await expect(status).toContainText('"status": "posted"');
      const ledger = await invoke("list_inventory_movements", { movement_type: "transfer" });
      expect(ledger.movements[0]).toMatchObject({ ...lot, source_location: "RAW-02", destination_location: "RAW-01", quantity: "2" });
      for (const [location, amount] of [["RAW-02", "0"], ["RAW-01", "2"]]) {
        const stock = await invoke("get_stock_balance", { item_id: "WIRE", location });
        expect(stock.balance.on_hand).toBe(amount);
      }
      return;
    }
    const reservation = { reservation_id: "RS-1" };
    await write("reserve_material", { ...reservation, ...lot, work_order: order.work_order, quantity: "1.5" });
    await write("pick_material", reservation);
    await write("dispatch_operation", { work_order: order.work_order, operation, work_center });
    await button("搜索工具").click();
    await page.getByLabel("匹配工具").selectOption("issue_material_to_operation");
    await page.getByLabel("JSON 参数").fill(JSON.stringify({ ...reservation, operation, expected_cursor: cursor, idempotency_key: "issue" }));
    await button("调用工具").click();
    await expect(status).toContainText('"status": "issued"');
    const stock = await invoke("get_material_availability", { item_id: "WIRE", revision: 1 });
    expect(stock.balance).toMatchObject({ on_hand: "0.5", reserved: "0", available: "0.5" });
    const ledger = await invoke("list_inventory_movements", { movement_type: "issue" });
    expect(ledger.movements).toHaveLength(1);
    expect(ledger.movements[0]).toMatchObject({ lot_id: "LOT-1", operation, work_order, quantity: "1.5" });
    if (report) {
      cursor = JSON.parse(await status.textContent()).cursor;
      await write("start_operation", { work_order, operation });
      await button("搜索工具").click();
      await page.getByLabel("匹配工具").selectOption("post_operation_report");
      await page.getByLabel("JSON 参数").fill(JSON.stringify({
        work_order, operation, quantity: 1, material_inputs: { "RS-1": "0.75" },
        expected_cursor: cursor, idempotency_key: "partial",
      }));
      await button("调用工具").click();
      await expect(status).toContainText('"remaining": 9');
      const posted = JSON.parse(await status.textContent());
      expect(posted.report.material_inputs).toEqual([
        { reservation_id: "RS-1", lot_id: "LOT-1", quantity: "0.75", unit: "m" },
      ]);
      const journal = await invoke("list_operation_reports", { work_order, operation });
      expect(journal.reports).toEqual([posted.report]);
      const after = await invoke("get_material_availability", { item_id: "WIRE", revision: 1 });
      expect(after.balance).toEqual(stock.balance);
      if (returns) {
        await button("搜索工具").click();
        await page.getByLabel("匹配工具").selectOption("return_unused_material");
        await page.getByLabel("JSON 参数").fill(JSON.stringify({
          ...reservation, movement_id: "RET-1", quantity: "0.75", reason: "unused",
          expected_cursor: posted.cursor, idempotency_key: "return",
        }));
        await button("调用工具").click();
        await expect(status).toContainText('"movement_id": "RET-1"');
        const returned = await invoke("list_inventory_movements", { movement_type: "return", location: "RAW-02" });
        expect(returned.movements[0]).toMatchObject({ ...lot, quantity: "0.75", location: "RAW-02" });
        const stockAfterReturn = await invoke("get_material_availability", { item_id: "WIRE", revision: 1 });
        expect(stockAfterReturn.balance).toMatchObject({ on_hand: "1.25", available: "1.25", reserved: "0" });
      }

    }
  });
}
