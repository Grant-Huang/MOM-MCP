import { test, expect } from "@playwright/test";

test("P7-UI sheet and harness exception flows share the scenario console", async ({ page }) => {
  await page.goto("/");
  await expect(page.getByRole("heading", { name: "五行业场景" })).toBeVisible();
  await page.getByRole("link", { name: "打开场景验收台" }).click();
  const status = page.getByRole("status");
  const button = (name) => page.getByRole("button", { name, exact: true });
  const tools = page.getByLabel("匹配工具");
  const selection = page.getByLabel("已发布场景");
  const run = page.getByLabel("当前运行");
  let key = 0;
  await button("加载场景").click();
  await expect(status).toContainText("端子压接与复测");

  async function open(id, tool) {
    await selection.selectOption(id);
    await button("打开场景").click();
    await expect(status).toContainText('"run_id"');
    const value = await run.inputValue();
    await button("搜索工具").click();
    await expect(tools.locator(`option[value="${tool}"]`)).toHaveCount(1);
    return value;
  }
  async function invoke(name, args, expected = '"ok": true') {
    await tools.selectOption(name);
    const payload = name.endsWith("_trace") ? args : { ...args, idempotency_key: `fabrication-${++key}` };
    await page.getByLabel("JSON 参数").fill(JSON.stringify(payload));
    await button("调用工具").click();
    await expect(status).toContainText(expected);
  }
  async function advance(cursor) {
    await button("推进一个事件").click();
    await expect(status).toContainText(`"event_cursor": ${cursor}`);
  }

  const sheet = await open("sheet-metal@1", "sheet_issue");
  await advance(1);
  await advance(2);
  await invoke("sheet_issue", { stock: "SHEET-FULL" }, "STOCK_UNAVAILABLE");
  await invoke("sheet_issue", { stock: "REM-OK" });
  await invoke("sheet_nest", {});
  await invoke("sheet_cut", { machine: "LASER-A" }, "MACHINE_UNAVAILABLE");
  await invoke("sheet_cut", { machine: "LASER-B" });
  await invoke("sheet_form", {});
  await invoke("sheet_inspect", { result: "pass" });
  await invoke("sheet_return", {}, '"phase": "complete"');
  await invoke("sheet_trace", { subject: "LOT-AL-2" }, '"area_mm2": 300');

  const harness = await open("wire-harness@1", "harness_cut");
  expect(harness).not.toBe(sheet);
  await invoke("harness_cut", { serial: "WIRE-01" });
  await invoke("harness_crimp", { serial: "WIRE-01", terminal_batch: "TERM-ALT" }, "QA-TERMINAL-12");
  await advance(1);
  await invoke("harness_sample", { event_cursor: 1 }, "WIRE-01");
  await invoke("harness_release", { serial: "WIRE-01" }, "TEST_PASS_REQUIRED");
  await invoke("harness_rework", { serial: "WIRE-01", terminal_batch: "TERM-STD", reason: "抽检返工" }, "HARNESS-2");
  await invoke("harness_test", { serial: "WIRE-01", result: "pass" }, '"phase": "passed"');
  await invoke("harness_release", { serial: "WIRE-01" }, '"phase": "released"');
  await invoke("harness_trace", { subject: "TERM-ALT" }, "TERM-STD");
  for (const id of [sheet, harness]) {
    await run.selectOption(id);
    await button("关闭运行").click();
    await expect(status).toContainText('"closed": true');
  }
});
