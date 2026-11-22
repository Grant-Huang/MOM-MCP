import { test, expect } from "@playwright/test";

test("P8B-UI Public order discovery and readiness", async ({ page }) => {
  await page.goto("/scenarios");
  const button = (name) => page.getByRole("button", { name, exact: true });
  const status = page.getByRole("status");
  await button("加载场景").click();
  await page.getByLabel("已发布场景").selectOption("cnc@7");
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
  const held = await invoke("hold_work_order", { work_order: "WO-NEW", reason: "work review", idempotency_key: "hold", expected_cursor: released.cursor }, '"status": "held"');
  await invoke("dispatch_operation", { work_order: "WO-NEW", operation: "machining", idempotency_key: "dispatch" }, "WORK_ORDER_HELD");
  await invoke("resume_work_order", { work_order: "WO-NEW", idempotency_key: "resume", expected_cursor: held.cursor }, '"status": "ready"');
  await invoke("dispatch_operation", { work_order: "WO-NEW", operation: "machining", idempotency_key: "dispatch" }, '"phase": "dispatched"');


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
