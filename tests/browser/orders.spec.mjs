import { test, expect } from "@playwright/test";

test("P8B-UI Public order discovery and readiness", async ({ page }) => {
  await page.goto("/scenarios");
  const button = (name) => page.getByRole("button", { name, exact: true });
  const status = page.getByRole("status");
  await button("加载场景").click();
  await page.getByLabel("已发布场景").selectOption("cnc@9");
  await button("打开场景").click();
  await expect(status).toContainText('"run_id"');
  await button("搜索工具").click();
  const tool = page.getByLabel("匹配工具");

  async function invoke(name, args, expected) {
    await tool.selectOption(name);
    await page.getByLabel("JSON 参数").fill(JSON.stringify(args));
    await button("调用工具").click();
    await expect(status).toContainText(expected);
    return JSON.parse(await status.textContent());
  }

  await invoke("list_work_orders", { limit: 1 }, '"next_after_work_order": "WO-1"');
  const { cursor } = await invoke("get_release_readiness", { work_order: "WO-1" }, '"ready": true');
  const draft = await invoke("create_work_order", { work_order: "WO-NEW", template_work_order: "WO-1", source_demand: "Demand-1", idempotency_key: "create", expected_cursor: cursor }, "Demand-1");
  await invoke("update_draft_order", { work_order: "WO-NEW", source_demand: "Demand-2", idempotency_key: "update", expected_cursor: draft.cursor }, "Demand-2");
  const cnc = await invoke("get_release_readiness", { work_order: "WO-NEW" }, '"ready": true');
  const released = await invoke("release_work_order", { work_order: "WO-NEW", idempotency_key: "release", expected_cursor: cnc.cursor }, '"status": "released"');
  await invoke("update_draft_order", { work_order: "WO-NEW", source_demand: "Demand-3", idempotency_key: "late-update", expected_cursor: released.cursor }, "INVALID_STATE");
  const revision = await invoke("revise_route", { route_version: "route-2", work_centers: ["MACH-01", "ASM-02", "QA-01"], idempotency_key: "revise", expected_cursor: released.cursor }, '"route_version": "route-2"');
  const held = await invoke("hold_work_order", { work_order: "WO-NEW", reason: "work review", idempotency_key: "hold", expected_cursor: revision.cursor }, '"status": "held"');
  await invoke("dispatch_operation", { work_order: "WO-NEW", operation: "machining", work_center: "MACH-01", idempotency_key: "dispatch" }, "WORK_ORDER_HELD");
  await invoke("resume_work_order", { work_order: "WO-NEW", idempotency_key: "resume", expected_cursor: held.cursor }, '"status": "ready"');
  await invoke("dispatch_operation", { work_order: "WO-NEW", operation: "machining", work_center: "MACH-01", idempotency_key: "dispatch" }, '"phase": "dispatched"');


  await invoke("issue_material", { work_order: "WO-NEW", batch: "LOT-1", idempotency_key: "issue" }, '"ok": true');
  for (const operation of ["machining", "assembly"]) {
    if (operation === "assembly") {
      await invoke("dispatch_operation", { work_order: "WO-NEW", operation, work_center: "ASM-02", idempotency_key: "wrong-center" }, "WORK_CENTER_MISMATCH");
      await invoke("dispatch_operation", { work_order: "WO-NEW", operation, work_center: "ASM-01", idempotency_key: "dispatch-assembly" }, '"phase": "dispatched"');
    }
    await invoke("start_operation", { work_order: "WO-NEW", operation, idempotency_key: `start-${operation}` }, '"phase": "running"');
    await invoke("report_operation", { work_order: "WO-NEW", operation, idempotency_key: `report-${operation}` }, '"remaining": 0');
  }
  await invoke("dispatch_operation", { work_order: "WO-NEW", operation: "inspection", work_center: "QA-01", idempotency_key: "dispatch-inspection" }, '"phase": "dispatched"');
  const inspected = await invoke("inspect_units", { work_order: "WO-NEW", result: "pass", idempotency_key: "inspect" }, '"phase": "complete"');
  await invoke("close_work_order", { work_order: "WO-NEW", expected_cursor: inspected.cursor, idempotency_key: "close" }, "COMPLETION_REQUIRED");
  const completed = await invoke("complete_work_order", { work_order: "WO-NEW", expected_cursor: inspected.cursor, idempotency_key: "complete" }, '"status": "completed"');
  await invoke("close_work_order", { work_order: "WO-NEW", expected_cursor: completed.cursor, idempotency_key: "close" }, '"status": "closed"');

  await button("关闭运行").click();
  await expect(status).toContainText('"closed": true');
  await page.getByLabel("已发布场景").selectOption("assembly-shortage@4");
  await button("打开场景").click();
  await expect(status).toContainText('"run_id"');
  await button("搜索工具").click();
  await invoke("get_release_readiness", { work_order: "WO-E" }, "MATERIAL_SHORTAGE");
  await invoke("assembly_substitute", { work_order: "WO-E", part: "DRIVE-ALT", idempotency_key: "substitute" }, "QA-DRIVE-017");
  const assembly = await invoke("get_release_readiness", { work_order: "WO-E" }, '"ready": true');
  const batch = { work_orders: ["WO-E", "WO-L", "WO-E"], idempotency_key: "batch", expected_cursor: assembly.cursor };
  const receipt = await invoke("release_work_orders", batch, "DUPLICATE_WORK_ORDER");
  expect(receipt.results.map((item) => item.status ?? item.code)).toEqual(["released", "MATERIAL_SHORTAGE", "DUPLICATE_WORK_ORDER"]);
  expect(await invoke("release_work_orders", batch, "DUPLICATE_WORK_ORDER")).toEqual(receipt);
  await invoke("assembly_dispatch", { work_order: "WO-E", cell: "CELL-A", idempotency_key: "dispatch" }, '"ok": true');
  await invoke("get_release_readiness", { work_order: "WO-E" }, "INVALID_STATE");
});

test("P8C-UI Operation detail and dispatch queue", async ({ page }) => {
  await page.goto("/scenarios");
  const button = (name) => page.getByRole("button", { name, exact: true });
  const status = page.getByRole("status");
  await button("加载场景").click();
  await page.getByLabel("已发布场景").selectOption("cnc@13");
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

  const order = { work_order: "WO-1" };
  await invoke("get_operation", { ...order, operation: "assembly" }, "PREDECESSOR_INCOMPLETE");
  await invoke("list_dispatch_queue", { work_center: "MACH-01" }, '"operations": []');
  const released = await invoke("release_work_order", { ...order, expected_cursor: 0, idempotency_key: "release" }, '"status": "released"');
  const ready = await invoke("list_dispatch_queue", { work_center: "MACH-01" }, '"status": "ready"');
  expect(ready.operations[0].operation).toBe("machining");
  const held = await invoke("hold_work_order", { ...order, expected_cursor: released.cursor, reason: "work review", idempotency_key: "hold" }, '"status": "held"');
  await invoke("list_dispatch_queue", { work_center: "MACH-01" }, '"operations": []');
  await invoke("resume_work_order", { ...order, expected_cursor: held.cursor, idempotency_key: "resume" }, '"status": "ready"');
  await invoke("list_dispatch_queue", { work_center: "MACH-01" }, '"work_order": "WO-1"');
  const capability = await invoke("get_resource_capability", { work_center: "MACH-01" }, '"available": true');
  expect(capability.resource.operations).toEqual(["machining"]);
  const priority = await invoke("set_dispatch_priority", { ...order, priority: 6, expected_cursor: held.cursor + 1, idempotency_key: "priority" }, '"priority": 6');
  await invoke("dispatch_operation", { ...order, operation: "machining", work_center: "ASM-01", expected_cursor: priority.cursor, idempotency_key: "dispatch" }, "WORK_CENTER_MISMATCH");
  const assigned = await invoke("dispatch_operation", { ...order, operation: "machining", work_center: "MACH-01", expected_cursor: priority.cursor, idempotency_key: "dispatch" }, '"phase": "dispatched"');
  await invoke("get_resource_capability", { work_center: "MACH-01" }, '"occupied_by": "WO-1"');
  const reassigned = await invoke("reassign_operation", { ...order, operation: "machining", work_center: "MACH-02", expected_cursor: assigned.cursor, idempotency_key: "reassign" }, '"phase": "dispatched"');
  await invoke("get_resource_capability", { work_center: "MACH-01" }, '"available": true');
  await invoke("get_resource_capability", { work_center: "MACH-02" }, '"occupied_by": "WO-1"');
  const issued = await invoke("issue_material", { ...order, batch: "LOT-1", idempotency_key: "issue", expected_cursor: reassigned.cursor }, '"batch": "LOT-1"');
  const op = { ...order, operation: "machining" };
  const started = await invoke("start_operation", { ...op, idempotency_key: "start", expected_cursor: issued.cursor }, '"phase": "running"');
  const paused = await invoke("pause_operation", { ...op, reason: "inspection", idempotency_key: "pause", expected_cursor: started.cursor }, '"phase": "paused"');
  await invoke("report_operation", { ...op, quantity: 1, idempotency_key: "report" }, "INVALID_STATE");
  await invoke("resume_operation", { ...op, idempotency_key: "resume-op", expected_cursor: paused.cursor }, '"phase": "running"');
  await invoke("list_dispatch_queue", { work_center: "MACH-01" }, '"operations": []');
});
