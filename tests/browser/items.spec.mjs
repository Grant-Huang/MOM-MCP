import { test, expect } from "@playwright/test";

test("P9A-03 Activate and read an item revision", async ({ page }) => {
  await page.goto("/scenarios");
  const button = (name) => page.getByRole("button", { name, exact: true });
  const status = page.getByRole("status");
  await button("加载场景").click();
  await page.getByLabel("已发布场景").selectOption("assembly@19");
  await button("打开场景").click();
  await expect(status).toContainText('"run_id"');
  await button("搜索工具").click();

  async function invoke(name, arguments_, expected) {
    await page.getByLabel("匹配工具").selectOption(name);
    await page.getByLabel("JSON 参数").fill(JSON.stringify(arguments_));
    await button("调用工具").click();
    await expect(status).toContainText(expected);
    return JSON.parse(await status.textContent());
  }

  const item = { item_id: "FRAME" };
  const write = (key, cursor, extra) => ({
    ...item, ...extra, idempotency_key: key, expected_cursor: cursor,
  });
  await invoke("create_item", write("item", 0, { base_unit: "wrong" }), "INVALID_ARGUMENT");
  await invoke("create_item", write("item", 0, { base_unit: "each" }), '"base_unit": "each"');
  await invoke("create_item_revision", write("draft", 1, { revision: 1, name: "Frame A" }), '"status": "draft"');
  await invoke("activate_revision", write("activate", 2, { revision_type: "item", revision: 1 }), '"status": "active"');
  const result = await invoke("get_item", item, '"active_revision": 1');
  expect(result.item).toEqual({ item_id: "FRAME", base_unit: "each", active_revision: 1 });
});

test("P9A-05 Validate and activate a BOM", async ({ page }) => {
  await page.goto("/scenarios");
  const button = (name) => page.getByRole("button", { name, exact: true });
  const status = page.getByRole("status");
  await button("加载场景").click();
  await page.getByLabel("已发布场景").selectOption("assembly@20");
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
  async function write(name, key, args, expected) {
    const result = await invoke(name, { ...args, expected_cursor: cursor, idempotency_key: key }, expected);
    if (result.ok) cursor = result.cursor;
    return result;
  }

  for (const [item_id, base_unit] of [["FRAME", "each"], ["WIRE", "m"]]) {
    await write("create_item", item_id, { item_id, base_unit }, '"base_unit"');
    await write("create_item_revision", `draft-${item_id}`, { item_id, revision: 1, name: item_id }, '"status": "draft"');
    await write("activate_revision", `publish-${item_id}`, { revision_type: "item", item_id, revision: 1 }, '"status": "active"');
  }
  const bom = { item_id: "FRAME", revision: 1 };
  await write("create_bom_revision", "bom", bom, '"status": "draft"');
  const wire = { revision: 1, quantity: 2, unit: "m" };
  await write("set_bom_components", "wrong", { ...bom, components: { WIRE: { ...wire, unit: "kg" } } }, "COMPONENT_UNIT_MISMATCH");
  await write("set_bom_components", "components", { ...bom, components: { WIRE: wire } }, '"quantity": 2');
  await invoke("validate_revision", { ...bom, revision_type: "bom" }, '"valid": true');
  await write("activate_revision", "publish-bom", { ...bom, revision_type: "bom" }, '"status": "active"');
  const result = await invoke("get_bom_revision", bom, '"bom_revision"');
  expect(result.bom_revision.components.WIRE).toMatchObject(wire);
  expect(result.bom_revision.status).toBe("active");
});

test("P9A-07 Configure and activate a route", async ({ page }) => {
  await page.goto("/scenarios");
  const button = (name) => page.getByRole("button", { name, exact: true });
  const status = page.getByRole("status");
  await button("加载场景").click();
  await page.getByLabel("已发布场景").selectOption("assembly@21");
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
    const result = await invoke(name, { ...args, expected_cursor: cursor, idempotency_key: name + cursor }, expected);
    if (result.ok) cursor = result.cursor;
    return result;
  }

  const item = { item_id: "FRAME", revision: 1 };
  const route = { ...item, revision_type: "route" };
  await write("create_item", { item_id: item.item_id, base_unit: "each" }, '"base_unit": "each"');
  await write("create_item_revision", { ...item, name: "Frame A" }, '"status": "draft"');
  await write("activate_revision", { ...item, revision_type: "item" }, '"status": "active"');
  await write("create_route_revision", item, '"status": "draft"');
  const final_test = { work_center: "TEST-1", predecessors: ["build"] };
  await write("set_route_operations", { ...item, operations: { final_test } }, "ROUTE_PREDECESSOR_NOT_FOUND");
  await write("set_route_operations", {
    ...item, operations: { build: { work_center: "CELL-A", predecessors: [] }, final_test },
  }, '"execution_order"');
  await invoke("validate_revision", route, '"valid": true');
  await write("activate_revision", route, '"status": "active"');
  const result = await invoke("get_route_revision", item, '"execution_order"');
  expect(result.route_revision.execution_order).toEqual(["build", "final_test"]);
  expect(result.route_revision.operations.final_test).toEqual(final_test);
});

test("P9A-09 Freeze a work order definition", async ({ page }) => {
  const gateway = async (name, arguments_) => {
    const response = await page.request.post("/api/scenarios", { data: { name, arguments: arguments_ } });
    expect(response.ok()).toBeTruthy();
    const result = await response.json();
    expect(result.ok, JSON.stringify(result)).toBe(true);
    return result;
  };

  await page.goto("/scenarios");
  const button = (name) => page.getByRole("button", { name, exact: true });
  const status = page.getByRole("status");
  await button("加载场景").click();
  await page.getByLabel("已发布场景").selectOption("assembly@22");
  await button("打开场景").click();
  await expect(status).toContainText('"run_id"');
  const { run_id } = JSON.parse(await status.textContent());
  const invoke = (name, arguments_) => gateway("call_tool", { run_id, name, arguments: arguments_ });
  let cursor = 0;
  const write = async (name, args) => {
    const result = await invoke(name, { ...args, expected_cursor: cursor, idempotency_key: name + cursor });
    cursor = result.cursor;
    return result;
  };
  for (const item_id of ["FRAME", "WIRE"]) {
    await write("create_item", { item_id, base_unit: "each" });
    await write("create_item_revision", { item_id, revision: 1, name: item_id });
    await write("activate_revision", { revision_type: "item", item_id, revision: 1 });
  }
  const frame = { item_id: "FRAME", revision: 1 };
  await write("create_bom_revision", frame);
  await write("set_bom_components", { ...frame, components: { WIRE: { revision: 1, quantity: 1, unit: "each" } } });
  await write("activate_revision", { ...frame, revision_type: "bom" });
  await write("create_route_revision", frame);
  await write("set_route_operations", { ...frame, operations: { build: { work_center: "CELL-A", predecessors: [] } } });
  await write("activate_revision", { ...frame, revision_type: "route" });
  const plan = { plan_id: "QA", revision: 1 };
  const characteristics = { width: { unit: "mm", lower_limit: 1, upper_limit: 2 } };
  await write("create_inspection_plan_revision", { ...plan, characteristics });
  await write("activate_inspection_plan_revision", plan);
  const release = { work_order: "WO-E", definition_selection: { item_id: "FRAME", inspection_plan_id: "QA" } };
  const ready = await invoke("get_release_readiness", release);
  expect(ready.ready).toBe(true);
  await write("release_work_order", { ...release, snapshot_fingerprint: ready.definition_snapshot.fingerprint });
  await write("create_inspection_plan_revision", { ...plan, revision: 2, characteristics });
  await write("activate_inspection_plan_revision", { ...plan, revision: 2 });

  await button("搜索工具").click();
  await page.getByLabel("匹配工具").selectOption("assess_engineering_change");
  await page.getByLabel("JSON 参数").fill(JSON.stringify({ work_order: "WO-E" }));
  await button("调用工具").click();
  await expect(status).toContainText('"requires_review": true');
  const comparison = JSON.parse(await status.textContent());
  expect(comparison.changed).toEqual(["inspection_plan"]);
  expect(comparison.frozen.inspection_plan.specification_version).toBe("QA@1");
  expect(comparison.current.inspection_plan.specification_version).toBe("QA@2");
});

test("P12-UI Freeze configuration-specific assembly requirements", ({ page }) => assemblyConfiguration(page, 30, "snapshot"));

test("P12-03 Trace a qualified assembly substitute", ({ page }) => assemblyConfiguration(page, 31, "substitute"));

test("P12-05 Observe contested assembly capacity", ({ page }) => assemblyConfiguration(page, 31, "shared-resources"));

test("P12-07 Read assembly as-built correction history", ({ page }) => assemblyConfiguration(page, 32, "as-built"));

async function assemblyConfiguration(page, version, flow) {
  const qualified = version >= 31;
  await page.goto("/scenarios");
  const button = (name) => page.getByRole("button", { name, exact: true });
  const status = page.getByRole("status");
  await button("加载场景").click();
  await page.getByLabel("已发布场景").selectOption(`assembly@${version}`);
  await button("打开场景").click();
  await expect(status).toContainText('"run_id"');
  const { run_id } = JSON.parse(await status.textContent());
  const invoke = async (name, arguments_) => {
    const response = await page.request.post("/api/scenarios", {
      data: { name: "call_tool", arguments: { run_id, name, arguments: arguments_ } },
    });
    expect(response.ok()).toBe(true);
    const result = await response.json();
    expect(result.ok, JSON.stringify(result)).toBe(true);
    return result;
  };
  let cursor = 0;
  const write = async (name, args) => {
    const result = await invoke(name, { ...args, expected_cursor: cursor, idempotency_key: name + cursor });
    cursor = result.cursor;
    return result;
  };
  for (const item_id of qualified ? ["FRAME", "BASE", "STD", "RUG", "ALT"] : ["FRAME", "BASE", "STD", "RUG"]) {
    await write("create_item", { item_id, base_unit: "each" });
    await write("create_item_revision", { item_id, revision: 1, name: item_id });
    await write("activate_revision", { item_id, revision: 1, revision_type: "item" });
  }
  const frame = { item_id: "FRAME", revision: 1 };
  await write("create_bom_revision", frame);
  await write("set_bom_components", { ...frame, components: {
    BASE: { revision: 1, quantity: 1, unit: "each" },
    STD: { revision: 1, quantity: 1, unit: "each", configurations: ["standard"],
      ...(qualified && { alternate: { item_id: "ALT", revision: 1, qualification_id: "QA-ALT-1" } }) },
    RUG: { revision: 1, quantity: 2, unit: "each", configurations: ["rugged"] },
  } });
  await write("activate_revision", { ...frame, revision_type: "bom" });
  await write("create_route_revision", frame);
  await write("set_route_operations", { ...frame, operations: { build: { work_center: "CELL-A", predecessors: [] } } });
  await write("activate_revision", { ...frame, revision_type: "route" });
  const plan = { plan_id: "QA", revision: 1 };
  await write("create_inspection_plan_revision", { ...plan, characteristics: { width: { unit: "mm", lower_limit: 1, upper_limit: 2 } } });
  await write("activate_inspection_plan_revision", plan);
  await button("搜索工具").click();
  await page.getByLabel("匹配工具").selectOption("get_order_snapshot");
  for (const [work_order, configuration, variant, quantity] of [["WO-E", "standard", "STD", "1"], ["WO-L", "rugged", "RUG", "2"]]) {
    const release = { work_order, definition_selection: { item_id: "FRAME", inspection_plan_id: "QA" } };
    const ready = await invoke("get_release_readiness", release);
    await write("release_work_order", { ...release, snapshot_fingerprint: ready.definition_snapshot.fingerprint });
    await page.getByLabel("JSON 参数").fill(JSON.stringify({ work_order }));
    await button("调用工具").click();
    await expect(status).toContainText(`"configuration": "${configuration}"`);
    const result = JSON.parse(await status.textContent());
    expect(Object.keys(result.snapshot.bom.components)).toEqual(["BASE", variant]);
    const requirements = await invoke("get_material_requirements", { work_order });
    expect(requirements.requirements).toContainEqual(expect.objectContaining({ item_id: variant, required: quantity }));
  }
  if (qualified) {
    for (const item of ["BASE", "ALT"]) {
      const receipt_id = "RC-" + item;
      const lot_id = "LOT-" + item;
      await write("create_receipt", { receipt_id, item_id: item, revision: 1, quantity: "1", requires_inspection: false });
      await write("assign_receipt_tracking", { receipt_id, lot_id, supplier_lot: "SUP-" + item, location: "RAW-02" });
      await write("post_receipt", { receipt_id });
      await write("reserve_material", { reservation_id: "RS-" + item, work_order: "WO-E", lot_id, quantity: "1" });
    }
    const operation = { work_order: "WO-E", operation: "build" };
    await write("dispatch_operation", { ...operation, work_center: "CELL-A" });
    for (const reservation_id of ["RS-BASE", "RS-ALT"]) {
      await write("pick_material", { reservation_id });
      await write("issue_material_to_operation", { reservation_id, operation: "build" });
    }
    await write("start_operation", operation);
    const { report } = await write("post_operation_report", {
      ...operation, quantity: 1, material_inputs: { "RS-BASE": "1", "RS-ALT": "1" },
    });
    expect(report.material_inputs).toContainEqual(expect.objectContaining({
      lot_id: "LOT-ALT", substitute: { required_item_id: "STD", qualification_id: "QA-ALT-1" },
    }));
    if (flow === "as-built") {
      await page.getByLabel("搜索词").fill("get_assembly_as_built");
      await button("搜索工具").click();
      await page.getByLabel("匹配工具").selectOption("get_assembly_as_built");
      await page.getByLabel("JSON 参数").fill(JSON.stringify({ serial_number: "UNIT-E-01" }));
      await button("调用工具").click();
      await expect(status).toContainText('"status": "active"');
      await write("reverse_operation_report", { work_order: "WO-E", report_id: report.id, reason: "correct actual inputs" });
      await button("调用工具").click();
      await expect(status).toContainText('"status": "reversed"');
      const manifest = JSON.parse(await status.textContent());
      expect(manifest.bom_revision).toBe(1);
      expect(manifest.builds[0].material_inputs).toContainEqual(expect.objectContaining({
        item_id: "ALT", lot_id: "LOT-ALT", revision: 1, substitute: { required_item_id: "STD", qualification_id: "QA-ALT-1" },
      }));
      return;
    }
    if (flow === "shared-resources") {
      await page.getByLabel("搜索词").fill("list_dispatch_queue");
      await button("搜索工具").click();
      await page.getByLabel("匹配工具").selectOption("list_dispatch_queue");
      await page.getByLabel("JSON 参数").fill(JSON.stringify({ work_center: "CELL-A" }));
      await button("调用工具").click();
      await expect(status).toContainText('"blocking": "RESOURCE_BUSY"');
      await write("complete_operation", operation);
      await button("调用工具").click();
      await expect(status).toContainText('"blocking": null');
      expect(JSON.parse(await status.textContent()).operations[0].work_order).toBe("WO-L");
      await write("dispatch_operation", { ...operation, work_order: "WO-L", work_center: "CELL-A" });
      return;
    }
    await page.getByLabel("匹配工具").selectOption("trace_serial_backward");
    await page.getByLabel("JSON 参数").fill(JSON.stringify({ serial_number: "UNIT-E-01" }));
    await button("调用工具").click();
    await expect(status).toContainText('"LOT-ALT"');
  }
}
