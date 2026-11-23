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
