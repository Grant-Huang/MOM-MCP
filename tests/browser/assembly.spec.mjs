import { test, expect } from "@playwright/test";

test("P5-UI assembly substitution test repair retest and delivery", async ({ page }) => {
  await page.goto("/scenarios");
  const status = page.getByRole("status");
  const button = (name) => page.getByRole("button", { name, exact: true });
  await button("加载场景").click();
  await expect(status).toContainText("缺料与返修");
  await page.getByLabel("已发布场景").selectOption("assembly-shortage@1");
  await button("打开场景").click();
  await expect(status).toContainText('"run_id"');
  await button("搜索工具").click();
  const tool = page.getByLabel("匹配工具");
  await expect(tool.locator('option[value="assembly_kit"]')).toHaveCount(1);
  let key = 0;
  async function invoke(name, payload, expected) {
    await tool.selectOption(name);
    await page.getByLabel("JSON 参数").fill(JSON.stringify(payload));
    await button("调用工具").click();
    await expect(status).toContainText(expected);
  }
  const order = (name, args, expected) => invoke(name,
    { work_order: "WO-E", idempotency_key: `assembly-${++key}`, ...args }, expected);
  async function advance(count) {
    for (let index = 0; index < count; index++) {
      await button("推进一个事件").click();
      await expect(status).toContainText('"stop"');
    }
  }
  await invoke("assembly_kit", { work_order: "WO-E" }, '"ready": false');
  await order("assembly_substitute", { part: "DRIVE-BAD" }, "SUBSTITUTE_NOT_QUALIFIED");
  await order("assembly_substitute", { part: "DRIVE-ALT" }, "QA-DRIVE-017");
  await order("assembly_dispatch", { cell: "CELL-A" }, '"phase": "dispatched"');
  await order("assembly_build", {}, '"phase": "queued"');
  await order("assembly_start_test", {}, '"phase": "testing"');
  await advance(3);
  await order("assembly_finish_test", {}, '"phase": "failed"');
  await order("assembly_deliver", {}, "TEST_PASS_REQUIRED");
  await order("assembly_repair", { reason: "连接器返修" }, '"phase": "queued"');
  await order("assembly_start_test", {}, '"phase": "testing"');
  await advance(1);
  await order("assembly_finish_test", {}, '"phase": "passed"');
  await order("assembly_deliver", {}, '"phase": "delivered"');
  await invoke("assembly_trace", { subject: "UNIT-E-01" }, "QA-DRIVE-017");
  await expect(status).toContainText("DRIVE-ALT");
  await button("关闭运行").click();
  await expect(status).toContainText('"closed": true');
});
