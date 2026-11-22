import { test, expect } from "@playwright/test";

test("P8-UI CNC partial reporting and reversal", async ({ page }) => {
  await page.goto("/scenarios");
  const button = (name) => page.getByRole("button", { name, exact: true });
  const status = page.getByRole("status");
  const tool = page.getByLabel("匹配工具");
  await button("加载场景").click();
  await expect(status).toContainText("泵体生产");
  await page.getByLabel("已发布场景").selectOption("cnc@2");
  await button("打开场景").click();
  await expect(status).toContainText('"run_id"');
  await button("搜索工具").click();
  await expect(tool.locator('option[value="report_operation"]')).toHaveCount(1);

  async function invoke(name, key, args, expected = '"ok": true') {
    await tool.selectOption(name);
    const payload = { work_order: "WO-1", ...args };
    if (key) payload.idempotency_key = key;
    await page.getByLabel("JSON 参数").fill(JSON.stringify(payload));
    await button("调用工具").click();
    await expect(status).toContainText(expected);
    return JSON.parse(await status.textContent());
  }
  await invoke("release_order", "release", {});
  await invoke("dispatch_operation", "dispatch", { operation: "machining" });
  await invoke("issue_material", "issue", { batch: "LOT-1" });
  await invoke("start_operation", "start", { operation: "machining" });
  await invoke("report_operation", "four", { operation: "machining", quantity: 4 }, '"remaining": 6');
  const second = await invoke("report_operation", "two", { operation: "machining", quantity: 2 }, '"remaining": 4');
  await invoke("reverse_report", "undo", { report_id: second.report.id, reason: "数量录错" }, '"phase": "running"');
  await invoke("mes_status", null, {}, '"on_hand": 6');
  await invoke("report_operation", "finish", { operation: "machining", quantity: 6 }, '"remaining": 0');
  await invoke("mes_status", null, {}, '"on_hand": 0');
  await button("关闭运行").click();
  await expect(status).toContainText('"closed": true');
});
