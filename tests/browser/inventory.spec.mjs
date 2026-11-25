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

for (const { title, scenario, work_order, operation, work_center, report, returns, transfer, reverse, receiptReversal, operationReversal, assemblyLedger, partial, wip, moveWip, genealogy, reverseWip, splitWip, outputReceipt, reconciliation, reconcileWip, qualityRecovery, replenish, crossShopHold, sharedSmt } of [
  { title: "P9C-03 Issue picked stock to an operation", scenario: "assembly@26", work_order: "WO-E", operation: "build", work_center: "CELL-A" },
  { title: "P9C-05 Record issued material in a partial report", scenario: "cnc@28", work_order: "WO-1", operation: "machining", work_center: "MACH-01", report: true },
  { title: "P9C-07 Return unused issued material", scenario: "cnc@29", work_order: "WO-1", operation: "machining", work_center: "MACH-01", report: true, returns: true },
  { title: "P9C-09 Post a stock transfer", scenario: "cnc@30", work_order: "WO-1", operation: "machining", work_center: "MACH-01", transfer: true },
  { title: "P9C-11 Reverse a posted stock transfer", scenario: "cnc@31", work_order: "WO-1", operation: "machining", work_center: "MACH-01", transfer: true, reverse: true },
  { title: "P9C-13 Reverse a posted receipt", scenario: "cnc@32", work_order: "WO-1", operation: "machining", work_center: "MACH-01", receiptReversal: true },
  { title: "P9C-15 Reverse issue and return movements", scenario: "cnc@33", work_order: "WO-1", operation: "machining", work_center: "MACH-01", operationReversal: true },
  { title: "P9C-17 Reconcile issued material in assembly", scenario: "assembly@27", work_order: "WO-E", operation: "build", work_center: "CELL-A", assemblyLedger: true },
  { title: "P9C-19 Transfer and reverse a partial lot", scenario: "cnc@34", work_order: "WO-1", operation: "machining", work_center: "MACH-01", partial: true },
  { title: "P9D-UI Find actual output in work in process", scenario: "cnc@35", work_order: "WO-1", operation: "machining", work_center: "MACH-01", wip: true },
  { title: "P9D-11 Reconcile CNC material and WIP", scenario: "cnc@40", work_order: "WO-1", operation: "machining", work_center: "MACH-01", moveWip: true, reconcileWip: true },
  { title: "P9D-15 Reconcile finished work order", scenario: "cnc@42", work_order: "WO-1", operation: "machining", work_center: "MACH-01", moveWip: true, outputReceipt: true, reconciliation: true },
  { title: "P9D-13 Receive serialized finished output", scenario: "cnc@41", work_order: "WO-1", operation: "machining", work_center: "MACH-01", moveWip: true, outputReceipt: true },
  { title: "P9D-17 Recover a quality-held output", scenario: "cnc@42", work_order: "WO-1", operation: "machining", work_center: "MACH-01", moveWip: true, outputReceipt: true, reconciliation: true, qualityRecovery: true },
  { title: "P9D-19 Replenish after a quality reinspection", scenario: "cnc@42", work_order: "WO-1", operation: "machining", work_center: "MACH-01", replenish: true },
  { title: "P9D-21 Hold WIP across workshops", scenario: "cnc@42", work_order: "WO-1", operation: "machining", work_center: "MACH-01", moveWip: true, crossShopHold: true },
  { title: "P9D-23 Trace SMT placement through the common stock ledger", scenario: "smt@3", work_order: "WO-A", operation: "placement", work_center: "SMT-1", sharedSmt: true },
  { title: "P9D-09 Split and merge WIP lots", scenario: "cnc@39", work_order: "WO-1", operation: "machining", work_center: "MACH-01", moveWip: true, splitWip: true },
  { title: "P9D-07 Reverse a WIP movement", scenario: "cnc@38", work_order: "WO-1", operation: "machining", work_center: "MACH-01", moveWip: true, reverseWip: true },
  { title: "P9D-05 Trace an issued lot and serial", scenario: "cnc@37", work_order: "WO-1", operation: "machining", work_center: "MACH-01", moveWip: true, genealogy: true },
  { title: "P9D-03 Move and consume a WIP lot", scenario: "cnc@36", work_order: "WO-1", operation: "machining", work_center: "MACH-01", moveWip: true },
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
      const expected = sharedSmt && ["smt_load", "smt_aoi", "smt_split"].includes(name) ? {} : { expected_cursor: cursor };
      const result = await invoke(name, { ...args, ...expected, idempotency_key: name + cursor });
      expect(result.ok, JSON.stringify(result)).toBe(true);
      cursor = result.cursor;
      return result;
    }

    for (const [item_id, base_unit] of [["FRAME", "each"], ["WIRE", sharedSmt ? "each" : "m"]]) {
      await write("create_item", { item_id, base_unit });
      await write("create_item_revision", { item_id, revision: 1, name: item_id });
      await write("activate_revision", { item_id, revision: 1, revision_type: "item" });
    }
    const frame = { item_id: "FRAME", revision: 1 };
    await write("create_bom_revision", frame);
    await write("set_bom_components", { ...frame, components: { WIRE: { revision: 1, quantity: sharedSmt ? 1 : 2, unit: sharedSmt ? "each" : "m" } } });
    await write("activate_revision", { ...frame, revision_type: "bom" });
    await write("create_route_revision", frame);
    const route = { [operation]: { work_center, predecessors: [] } };
    if (moveWip) route.assembly = { work_center: "ASM-01", predecessors: ["machining"] };
    if (outputReceipt) route.inspection = { work_center: "QA-01", predecessors: ["assembly"] };
    await write("set_route_operations", { ...frame, operations: route });
    await write("activate_revision", { ...frame, revision_type: "route" });
    const plan = { plan_id: "QA", revision: 1 };
    await write("create_inspection_plan_revision", { ...plan, characteristics: { width: { unit: "mm", lower_limit: 1, upper_limit: 2 } } });
    await write("activate_inspection_plan_revision", plan);
    const order = { work_order, definition_selection: { item_id: "FRAME", inspection_plan_id: "QA" } };
    const ready = await invoke("get_release_readiness", order);
    await write("release_work_order", { ...order, snapshot_fingerprint: ready.definition_snapshot.fingerprint });
    const receipt = { receipt_id: "RC-1" };
    const lot = { lot_id: sharedSmt ? "REEL-C" : "LOT-1" };
    await write("create_receipt", { ...receipt, item_id: "WIRE", revision: 1, quantity: "2", requires_inspection: false });
    await write("assign_receipt_tracking", { ...receipt, ...lot, supplier_lot: "SUP-42", location: "RAW-02" });
    await write("post_receipt", receipt);
    if (sharedSmt) {
      const reservation_id = "RS-1";
      await write("reserve_material", { ...lot, reservation_id, work_order, quantity: "2" });
      await write("pick_material", { reservation_id });
      await write("issue_material_to_operation", { reservation_id, operation });
      await write("smt_load", { slot: "F1", reel: lot.lot_id });
      const placed = await write("smt_place", { panel: "P-A", slot: "F1", reservation_id });
      await write("smt_aoi", { panel: "P-A", result: "pass" });
      await write("smt_split", { panel: "P-A" });
      await button("搜索工具").click();
      await page.getByLabel("匹配工具").selectOption("trace_serial_backward");
      await page.getByLabel("JSON 参数").fill(JSON.stringify({ serial_number: "A-01" }));
      await button("调用工具").click();
      await expect(status).toContainText('"reservation_id": "RS-1"');
      const trace = JSON.parse(await status.textContent());
      expect(trace.edges[0]).toMatchObject({ source: { id: lot.lot_id }, target: { id: placed.report_id }, quantity: "2" });
      expect((await invoke("get_stock_balance", { item_id: "WIRE", location: "RAW-02" })).balance.on_hand).toBe("0");
      return;
    }

    if (partial) {
      await write("create_stock_transfer", {
        ...lot, transfer_id: "TR-1", child_lot_id: "LOT-2", quantity: "0.75",
        source_location: "RAW-02", destination_location: "RAW-01", reason: "split stock",
      });
      await button("搜索工具").click();
      await page.getByLabel("匹配工具").selectOption("post_stock_transfer");
      await page.getByLabel("JSON 参数").fill(JSON.stringify({ transfer_id: "TR-1", expected_cursor: cursor, idempotency_key: "post-split" }));
      await button("调用工具").click();
      await expect(status).toContainText('"child_lot_id": "LOT-2"');
      const posted = JSON.parse(await status.textContent());
      const child = await invoke("get_lot", { lot_id: "LOT-2" });
      expect(child.lot).toMatchObject({ parent_lot_id: "LOT-1", quantity: "0.75", location: "RAW-01" });
      cursor = posted.cursor;
      await write("reverse_inventory_movement", { movement_type: "transfer", movement_id: "TR-1", reversal_id: "TR-2", reason: "incorrect move" });
      const source = await invoke("get_stock_balance", { item_id: "WIRE", location: "RAW-02" });
      expect(source.balance.on_hand).toBe("2");
      expect((await invoke("get_lot", { lot_id: "LOT-2" })).code).toBe("LOT_NOT_FOUND");
      return;
    }
    if (receiptReversal) {
      await button("搜索工具").click();
      await page.getByLabel("匹配工具").selectOption("reverse_inventory_movement");
      await page.getByLabel("JSON 参数").fill(JSON.stringify({
        movement_type: "receipt", movement_id: "RC-1", reversal_id: "REV-1", reason: "receipt entry error",
        expected_cursor: cursor, idempotency_key: "reverse-receipt",
      }));
      await button("调用工具").click();
      await expect(status).toContainText('"reversal_id": "REV-1"');
      const original = await invoke("list_inventory_movements", { movement_type: "receipt" });
      expect(original.movements[0]).toMatchObject({ receipt_id: "RC-1", lot_id: "LOT-1", quantity: "2" });
      const ledger = await invoke("list_inventory_movements", { movement_type: "receipt_reversal" });
      expect(ledger.movements[0]).toMatchObject({ receipt_id: "RC-1", reversal_id: "REV-1", quantity: "2", location: "RAW-02" });
      const stock = await invoke("get_stock_balance", { item_id: "WIRE", location: "RAW-02" });
      expect(stock.balance).toMatchObject({ on_hand: "0", available: "0" });
      return;
    }
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
      if (reverse) {
        const posted = JSON.parse(await status.textContent());
        await button("搜索工具").click();
        await page.getByLabel("匹配工具").selectOption("reverse_inventory_movement");
        await page.getByLabel("JSON 参数").fill(JSON.stringify({
          movement_type: "transfer", movement_id: "TR-1", reversal_id: "TR-2", reason: "wrong destination",
          expected_cursor: posted.cursor, idempotency_key: "reverse-transfer",
        }));
        await button("调用工具").click();
        await expect(status).toContainText('"reverses_transfer_id": "TR-1"');
        const after = await invoke("list_inventory_movements", { movement_type: "transfer" });
        expect(after.movements[0]).toEqual(ledger.movements[0]);
        expect(after.movements[1]).toMatchObject({
          reverses_transfer_id: "TR-1", source_location: "RAW-01", destination_location: "RAW-02", quantity: "2",
        });
        const stock = await invoke("get_stock_balance", { item_id: "WIRE", location: "RAW-02" });
        expect(stock.balance.on_hand).toBe("2");
      }
      return;
    }
    const reservation = { reservation_id: "RS-1" };
    await write("reserve_material", { ...reservation, ...lot, work_order: order.work_order, quantity: assemblyLedger ? "2" : "1.5" });
    await write("pick_material", reservation);
    await write("dispatch_operation", { work_order: order.work_order, operation, work_center });
    await button("搜索工具").click();
    await page.getByLabel("匹配工具").selectOption("issue_material_to_operation");
    await page.getByLabel("JSON 参数").fill(JSON.stringify({ ...reservation, operation, expected_cursor: cursor, idempotency_key: "issue" }));
    await button("调用工具").click();
    await expect(status).toContainText('"status": "issued"');
    const stock = await invoke("get_material_availability", { item_id: "WIRE", revision: 1 });
    expect(stock.balance).toMatchObject({ on_hand: assemblyLedger ? "0" : "0.5", reserved: "0", available: assemblyLedger ? "0" : "0.5" });
    const ledger = await invoke("list_inventory_movements", { movement_type: "issue" });
    expect(ledger.movements).toHaveLength(1);
    expect(ledger.movements[0]).toMatchObject({ lot_id: "LOT-1", operation, work_order, quantity: assemblyLedger ? "2" : "1.5" });
    if (replenish) {
      cursor = JSON.parse(await status.textContent()).cursor;
      await write("start_operation", { work_order, operation });
      await write("post_operation_report", { work_order, operation, quantity: 4, material_inputs: { "RS-1": "0.5" } });
      await write("create_receipt", { receipt_id: "RC-2", item_id: "WIRE", revision: 1, quantity: "1", requires_inspection: false });
      await write("assign_receipt_tracking", { receipt_id: "RC-2", lot_id: "LOT-2", supplier_lot: "SUP-2", location: "RAW-01" });
      await write("post_receipt", { receipt_id: "RC-2" });
      await write("reserve_material", { reservation_id: "RS-2", work_order, lot_id: "LOT-2", quantity: "1" });
      await write("pick_material", { reservation_id: "RS-2" });
      const inspect = async (inspection_id, value, previous = {}) => {
        await write("create_inspection", { work_order, inspection_id, specification_version: "QA@1", ...previous });
        await write("record_measurement", { inspection_id, characteristic: "width", value, unit: "mm", lower_limit: 1, upper_limit: 2 });
        return write("complete_inspection", { inspection_id });
      };
      expect((await inspect("IN-FAIL", 3)).inspection.status).toBe("failed");
      const review = { inspection_id: "IN-FAIL", responsible: "QA-01", reason: "in-process review", evidence: "QA-LOG" };
      await write("open_nonconformance", review);
      await write("place_quality_hold", review);
      async function issueFromPage() {
        await button("搜索工具").click();
        await page.getByLabel("匹配工具").selectOption("issue_material_to_operation");
        await page.getByLabel("JSON 参数").fill(JSON.stringify({
          reservation_id: "RS-2", operation, expected_cursor: cursor, idempotency_key: "replenish",
        }));
        await button("调用工具").click();
      }
      await issueFromPage();
      await expect(status).toContainText('"code": "QUALITY_HOLD"');
      expect((await inspect("IN-PASS", 1.5, { reinspection_of: "IN-FAIL" })).inspection.status).toBe("passed");
      await write("disposition_nonconformance", { ...review, decision: "reinspection", reinspection_id: "IN-PASS" });
      await write("release_quality_hold", review);
      await issueFromPage();
      await expect(status).toContainText('"status": "issued"');
      cursor = JSON.parse(await status.textContent()).cursor;
      const output = await write("post_operation_report", { work_order, operation, quantity: 6, material_inputs: { "RS-2": "1" } });
      expect(output.report.material_inputs).toMatchObject([{ reservation_id: "RS-2", lot_id: "LOT-2", quantity: "1" }]);
      return;
    }
    if (moveWip) {
      cursor = JSON.parse(await status.textContent()).cursor;
      await write("start_operation", { work_order, operation });
      const source = await write("post_operation_report", { work_order, operation, quantity: 10, material_inputs: { "RS-1": "1.5" } });
      await write("complete_operation", { work_order, operation });
      await button("搜索工具").click();
      if (splitWip) {
        const callFromPage = async (name, args, expected) => {
          await page.getByLabel("匹配工具").selectOption(name);
          await page.getByLabel("JSON 参数").fill(JSON.stringify({
            ...args, expected_cursor: cursor, idempotency_key: name,
          }));
          await button("调用工具").click();
          await expect(status).toContainText(expected);
          const result = JSON.parse(await status.textContent());
          expect(result.ok).toBe(true);
          cursor = result.cursor;
          return result;
        };
        const parent = { work_order, wip_id: source.report.id };
        const split = await callFromPage("split_wip", {
          ...parent, selected_serial_numbers: source.report.serial_numbers.slice(0, 3), reason: "stage separate bins",
        }, '"split_cursor"');
        const [selected, remaining] = split.partition.children;
        expect(selected.serial_numbers).toEqual(source.report.serial_numbers.slice(0, 3));
        expect(remaining.serial_numbers).toEqual(source.report.serial_numbers.slice(3));
        expect((await invoke("get_wip", parent)).lots[0].status).toBe("split");
        await callFromPage("merge_wip", {
          ...parent, split_cursor: split.partition.split_cursor, reason: "recombine",
        }, '"merge_reason": "recombine"');
        expect((await invoke("get_wip", parent)).lots[0]).toMatchObject({ quantity: 10, status: "available" });
        const history = await invoke("get_wip", { work_order, wip_id: selected.id });
        expect(history.lots[0]).toMatchObject({ status: "merged", parent_wip_id: source.report.id });
        return;
      }
      await page.getByLabel("匹配工具").selectOption("move_wip");
      await page.getByLabel("JSON 参数").fill(JSON.stringify({
        work_order, wip_id: source.report.id, destination_operation: "assembly", reason: "handoff",
        expected_cursor: cursor, idempotency_key: "move",
      }));
      await button("调用工具").click();
      await expect(status).toContainText('"destination_operation": "assembly"');
      const moved = JSON.parse(await status.textContent());
      cursor = moved.cursor;
      if (reverseWip) {
        await button("搜索工具").click();
        await page.getByLabel("匹配工具").selectOption("reverse_wip_movement");
        await page.getByLabel("JSON 参数").fill(JSON.stringify({
          work_order, wip_id: source.report.id, movement_cursor: moved.movement.cursor,
          reason: "incorrect handoff", expected_cursor: cursor, idempotency_key: "withdraw",
        }));
        await button("调用工具").click();
        await expect(status).toContainText('"reversed_cursor"');
        const current = await invoke("get_wip", { work_order, wip_id: source.report.id });
        expect(current.lots[0].status).toBe("available");
        expect(current.lots[0].movement).toBeUndefined();
        expect(current.lots[0].movement_reversals[0].original_movement).toEqual(moved.movement);
        return;
      }
      await write("dispatch_operation", { work_order, operation: "assembly", work_center: "ASM-01" });
      await write("start_operation", { work_order, operation: "assembly" });
      if (crossShopHold) {
        await write("create_inspection", { work_order, inspection_id: "SHOP-FAIL", specification_version: "QA@1" });
        await write("record_measurement", { inspection_id: "SHOP-FAIL", characteristic: "width", value: 3, unit: "mm", lower_limit: 1, upper_limit: 2 });
        expect((await write("complete_inspection", { inspection_id: "SHOP-FAIL" })).inspection.status).toBe("failed");
        const review = { inspection_id: "SHOP-FAIL", responsible: "QA-01", reason: "workshop isolation", evidence: "QA-LOG" };
        await write("open_nonconformance", review);
        await write("place_quality_hold", review);
        await button("搜索工具").click();
        await page.getByLabel("匹配工具").selectOption("post_operation_report");
        await page.getByLabel("JSON 参数").fill(JSON.stringify({
          work_order, operation: "assembly", quantity: 10, wip_inputs: [source.report.id],
          expected_cursor: cursor, idempotency_key: "blocked-shop-report",
        }));
        await button("调用工具").click();
        await expect(status).toContainText('"code": "QUALITY_HOLD"');
        const held = await invoke("get_wip", { work_order, wip_id: source.report.id });
        expect(held.lots[0]).toMatchObject({ status: "held", blocking: "QUALITY_HOLD", movement: { destination_operation: "assembly" } });
        expect(held.lots[0].consumed_by).toBeUndefined();
        const other = await invoke("reconcile_work_order", { work_order: "WO-2" });
        expect(other.settlement.blocking).not.toContain("QUALITY_HOLD");
        return;
      }
      const output = await write("post_operation_report", { work_order, operation: "assembly", quantity: 10, wip_inputs: [source.report.id] });
      const current = await invoke("get_wip", { work_order, wip_id: source.report.id });
      expect(current.lots[0]).toMatchObject({ status: "consumed", consumed_by: output.report.id });
      expect(output.report.serial_numbers).toEqual(source.report.serial_numbers);
      if (reconcileWip) {
        await button("搜索工具").click();
        await page.getByLabel("匹配工具").selectOption("reconcile_work_order");
        await page.getByLabel("JSON 参数").fill(JSON.stringify({ work_order }));
        await button("调用工具").click();
        await expect(status).toContainText('"balanced": true');
        const account = JSON.parse(await status.textContent());
        expect(account.wip).toMatchObject({ machined: 10, consumed: 10, assembly_reported: 10 });
        expect(account.issues[0]).toMatchObject({ issued: "1.5", consumed: "1.5", unconsumed: "0" });
      }
      if (outputReceipt) {
        await write("complete_operation", { work_order, operation: "assembly" });
        await write("dispatch_operation", { work_order, operation: "inspection", work_center: "QA-01" });
        await write("inspect_units", { work_order, result: "pass" });
        await write("complete_work_order", { work_order });
        if (qualityRecovery) {
          const inspect = async (inspection_id, value, previous = {}) => {
            await write("create_inspection", { work_order, inspection_id, specification_version: "QA@1", ...previous });
            await write("record_measurement", { inspection_id, characteristic: "width", value, unit: "mm", lower_limit: 1, upper_limit: 2 });
            return write("complete_inspection", { inspection_id });
          };
          expect((await inspect("FINAL-FAIL", 3)).inspection.status).toBe("failed");
          const review = { inspection_id: "FINAL-FAIL", responsible: "QA-01", reason: "final review", evidence: "QA-LOG" };
          await write("open_nonconformance", review);
          await write("place_quality_hold", review);
          await button("搜索工具").click();
          await page.getByLabel("匹配工具").selectOption("receive_work_order_output");
          await page.getByLabel("JSON 参数").fill(JSON.stringify({
            work_order, receipt_id: "FIN-1", lot_id: "FIN-LOT-1", location: "RAW-02",
            expected_cursor: cursor, idempotency_key: "finish-receipt",
          }));
          await button("调用工具").click();
          await expect(status).toContainText('"code": "QUALITY_HOLD"');
          const blocked = await invoke("reconcile_work_order", { work_order });
          expect(blocked.settlement).toMatchObject({ quality: "QUALITY_HOLD", output_receipts: [] });
          expect((await inspect("FINAL-PASS", 1.5, { reinspection_of: "FINAL-FAIL" })).inspection.status).toBe("passed");
          await write("disposition_nonconformance", { ...review, decision: "reinspection", reinspection_id: "FINAL-PASS" });
          await write("release_quality_hold", review);
        }
        await button("搜索工具").click();
        await page.getByLabel("匹配工具").selectOption("receive_work_order_output");
        const location = "RAW-02";
        const lot_id = "FIN-LOT-1";
        await page.getByLabel("JSON 参数").fill(JSON.stringify({
          work_order, receipt_id: "FIN-1", lot_id, location, expected_cursor: cursor, idempotency_key: "finish-receipt",
        }));
        await button("调用工具").click();
        await expect(status).toContainText('"receipt_id": "FIN-1"');
        const posted = JSON.parse(await status.textContent());
        expect(posted.receipt).toMatchObject({ work_order, quantity: "10", serial_numbers: output.report.serial_numbers });
        cursor = posted.cursor;
        expect((await invoke("get_lot", { lot_id })).lot).toEqual(posted.receipt);
        expect((await invoke("get_stock_balance", { item_id: "FRAME", revision: 1, location })).balance.on_hand).toBe("10");
        expect((await invoke("list_inventory_movements", { item_id: "FRAME", location })).movements).toEqual([posted.receipt]);
        await write("close_work_order", { work_order });
        if (reconciliation) {
          await button("搜索工具").click();
          await page.getByLabel("匹配工具").selectOption("reconcile_work_order");
          await page.getByLabel("JSON 参数").fill(JSON.stringify({ work_order }));
          await button("调用工具").click();
          await expect(status).toContainText('"closed": true');
          const reconciled = JSON.parse(await status.textContent());
          expect(reconciled.balanced).toBe(true);
          expect(reconciled.settlement).toMatchObject({ cursor, quality: "released", differences: [], blocking: [],
            production: { machined: 10, assembled: 10, wip_open: 0, received: 10 },
            output_receipts: [{ lot_id, quantity: "10", lot_on_hand: "10", reversed: false }],
          });
        }
      }
      if (genealogy) {
        const serial_number = source.report.serial_numbers[0];
        await button("搜索工具").click();
        await page.getByLabel("匹配工具").selectOption("trace_lot_forward");
        await page.getByLabel("JSON 参数").fill(JSON.stringify({ lot_id: "LOT-1" }));
        await button("调用工具").click();
        await expect(status).toContainText('"kind": "wip_consumption"');
        const forward = JSON.parse(await status.textContent());
        expect(forward.edges.some((edge) => edge.kind === "serial_output" && edge.target.id === serial_number)).toBe(true);
        const backward = await invoke("trace_serial_backward", { serial_number });
        expect(backward.edges[0]).toMatchObject({
          kind: "material_input", source: { id: "LOT-1" }, reservation_id: "RS-1", quantity: "1.5",
        });
      }
      return;
    }
    if (wip) {
      cursor = JSON.parse(await status.textContent()).cursor;
      await write("start_operation", { work_order, operation });
      const posted = await write("post_operation_report", { work_order, operation, quantity: 1, material_inputs: { "RS-1": "0.75" } });
      await button("搜索工具").click();
      await page.getByLabel("匹配工具").selectOption("get_wip");
      await page.getByLabel("JSON 参数").fill(JSON.stringify({ work_order }));
      await button("调用工具").click();
      await expect(status).toContainText('"status": "available"');
      const output = JSON.parse(await status.textContent()).lots[0];
      expect(output).toMatchObject({ report_id: posted.report.id, quantity: 1, serial_numbers: posted.report.serial_numbers, material_inputs: posted.report.material_inputs });
      await write("reverse_operation_report", { work_order, report_id: output.report_id, reason: "wrong actual" });
      const reversed = await invoke("get_wip", { work_order, wip_id: output.wip_id });
      expect(reversed.lots[0]).toMatchObject({ status: "reversed", report_id: posted.report.id });
      return;
    }
    if (assemblyLedger) {
      cursor = JSON.parse(await status.textContent()).cursor;
      await write("start_operation", { work_order, operation });
      await button("搜索工具").click();
      await page.getByLabel("匹配工具").selectOption("post_operation_report");
      await page.getByLabel("JSON 参数").fill(JSON.stringify({
        work_order, operation, quantity: 1, material_inputs: { "RS-1": "2" },
        expected_cursor: cursor, idempotency_key: "assembly-report",
      }));
      await button("调用工具").click();
      await expect(status).toContainText('"remaining": 0');
      const actual = JSON.parse(await status.textContent());
      expect(actual.report.material_inputs).toMatchObject([{ lot_id: "LOT-1", quantity: "2" }]);
      cursor = actual.cursor;
      await write("reverse_operation_report", { work_order, report_id: actual.report.id, reason: "incorrect report" });
      await write("return_unused_material", { ...reservation, movement_id: "RET-1", quantity: "2", reason: "unused" });
      const balance = await invoke("get_stock_balance", { item_id: "WIRE", location: "RAW-02" });
      expect(balance.balance).toMatchObject({ on_hand: "2", available: "2" });
      return;
    }
    if (operationReversal) {
      cursor = JSON.parse(await status.textContent()).cursor;
      await write("return_unused_material", { ...reservation, movement_id: "RET-1", quantity: "0.5", reason: "unused" });
      await button("搜索工具").click();
      await page.getByLabel("匹配工具").selectOption("reverse_inventory_movement");
      await page.getByLabel("JSON 参数").fill(JSON.stringify({
        movement_type: "return", movement_id: "RET-1", reversal_id: "REV-RETURN", reason: "incorrect return",
        expected_cursor: cursor, idempotency_key: "reverse-return",
      }));
      await button("调用工具").click();
      await expect(status).toContainText('"reversal_id": "REV-RETURN"');
      cursor = JSON.parse(await status.textContent()).cursor;
      await write("reverse_inventory_movement", {
        movement_type: "issue", movement_id: "RS-1", reversal_id: "REV-ISSUE", reason: "incorrect issue",
      });
      const corrections = await invoke("list_inventory_movements", { movement_type: "operation_reversal" });
      expect(corrections.movements.map(({ movement_type, quantity }) => [movement_type, quantity])).toEqual([["issue", "1.5"], ["return", "0.5"]]);
      const finalStock = await invoke("get_stock_balance", { item_id: "WIRE", location: "RAW-02" });
      expect(finalStock.balance).toMatchObject({ on_hand: "2", available: "2" });
      return;
    }
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
