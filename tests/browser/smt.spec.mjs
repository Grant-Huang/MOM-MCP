import { test, expect } from "@playwright/test";

test("P4-UI SMT recall isolation rework and genealogy", async ({ page }) => {
  await page.goto("/scenarios");
  const status = page.getByRole("status");
  const button = (name) => page.getByRole("button", { name, exact: true });
  await button("加载场景").click();
  await expect(status).toContainText("贴装与召回");
  await page.getByLabel("已发布场景").selectOption("smt@2");
  await button("打开场景").click();
  await expect(status).toContainText('"run_id"');
  await button("搜索工具").click();
  const tool = page.getByLabel("匹配工具");
  await expect(tool.locator('option[value="smt_release"]')).toHaveCount(1);
  let key = 0;
  async function invoke(name, args, expected = '"ok": true') {
    await tool.selectOption(name);
    const payload = ["smt_trace", "smt_status"].includes(name) ? args : { ...args, idempotency_key: `smt-ui-${++key}` };
    await page.getByLabel("JSON 参数").fill(JSON.stringify(payload));
    await button("调用工具").click();
    await expect(status).toContainText(expected);
  }

  await invoke("smt_release", { work_order: "WO-A" });
  await invoke("smt_load", { reel: "REEL-R", slot: "F1" });
  await invoke("smt_place", { panel: "P-A", slot: "F1" });
  await invoke("smt_aoi", { panel: "P-A", result: "pass" });
  await invoke("smt_split", { panel: "P-A" }, "A-01");
  for (let cursor = 1; cursor <= 2; cursor++) {
    await button("推进一个事件").click();
    await expect(status).toContainText(`"event_cursor": ${cursor}`);
  }
  await button("读取事件").click();
  await expect(status).toContainText("supplier_recall");
  const event = JSON.parse(await status.textContent()).events.find(({ kind }) => kind === "supplier_recall");
  await invoke("smt_isolate", { event_cursor: event.cursor }, "A-01");
  await invoke("smt_finish", { serial: "A-01" }, "QUALITY_HOLD");
  await invoke("smt_rework", { serial: "A-01", replacement_reel: "REEL-C", reason: "召回换料" }, "awaiting_retest");
  await invoke("smt_retest", { serial: "A-01", result: "pass" }, '"quality": "clear"');
  await invoke("smt_finish", { serial: "A-01" }, '"finished": true');
  await invoke("smt_scrap", { serial: "A-02", reason: "不可返工" }, '"quality": "scrapped"');
  await invoke("smt_trace", { subject: "A-01" }, "REEL-R");
  await expect(status).toContainText("REEL-C");
  await button("关闭运行").click();
  await expect(status).toContainText('"closed": true');
});
