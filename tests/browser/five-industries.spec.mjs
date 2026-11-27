import { test, expect } from "@playwright/test";

test("P15-UI Switch five industry runs without losing work", async ({ page }) => {
  await page.goto("/scenarios");
  const button = (name) => page.getByRole("button", { name, exact: true });
  const status = page.getByRole("status");
  const selection = page.getByLabel("已发布场景");
  const current = page.getByLabel("当前运行");
  const scenarios = [
    ["cnc@49", "get_work_order", { work_order: "WO-1" }, '"work_order": "WO-1"'],
    ["smt@9", "smt_status", { work_order: "WO-A" }, '"released": false'],
    ["assembly@33", "get_assembly_as_built", { serial_number: "UNIT-E-01" }, '"serial_number": "UNIT-E-01"'],
    ["sheet-metal@6", "create_sheet_nest", { nest_id: "UI-NEST", stock: "SHEET-FULL",
      work_orders: ["WO-SHEET-A", "WO-SHEET-B"], idempotency_key: "shared" }, '"status": "planned"'],
    ["wire-harness@6", "cut_harness_batch", { batch_id: "UI-CUT", wire_lot: "WIRE-CU-1",
      serial_numbers: ["WIRE-01"], idempotency_key: "shared" }, '"input_mm": 660'],
  ];
  const runs = [];
  await button("加载场景").click();
  for (const [version, tool, args, evidence] of scenarios) {
    await selection.selectOption(version);
    await button("打开场景").click();
    await expect.poll(() => current.inputValue()).not.toBe(runs.at(-1) ?? "");
    runs.push(await current.inputValue());
    await page.getByLabel("搜索词").fill(tool);
    await button("搜索工具").click();
    await page.getByLabel("匹配工具").selectOption(tool);
    await page.getByLabel("JSON 参数").fill(JSON.stringify(args));
    await button("调用工具").click();
    await expect(status).toContainText(evidence);
  }
  expect(new Set(runs).size).toBe(5);
  await current.selectOption(runs[3]);
  await page.getByLabel("搜索词").fill("get_sheet_nest");
  await button("搜索工具").click();
  await page.getByLabel("匹配工具").selectOption("get_sheet_nest");
  await page.getByLabel("JSON 参数").fill(JSON.stringify({ nest_id: "UI-NEST" }));
  await button("调用工具").click();
  await expect(status).toContainText('"nest_id": "UI-NEST"');
  expect(JSON.parse(await status.textContent()).nest.placements["WO-SHEET-B"].x_mm).toBe(30);
});
