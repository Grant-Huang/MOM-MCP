import { test, expect } from "@playwright/test";

test("P3-UI CNC work order through inspection closure and genealogy", async ({ page }) => {
  await page.goto("/scenarios");
  const status = page.getByRole("status");
  const button = (name) => page.getByRole("button", { name, exact: true });
  await button("加载场景").click();
  await expect(status).toContainText("泵体生产");
  await page.getByLabel("已发布场景").selectOption("cnc@2");
  await button("打开场景").click();
  await expect(status).toContainText('"run_id"');
  await button("搜索工具").click();
  const tool = page.getByLabel("匹配工具");
  await expect(tool.locator('option[value="release_order"]')).toHaveCount(1);
  let key = 0;
  async function invoke(name, args, expected = '"ok": true') {
    await tool.selectOption(name);
    await page.getByLabel("JSON 参数").fill(JSON.stringify(args));
    await button("调用工具").click();
    await expect(status).toContainText(expected);
  }
  async function order(name, extras = {}, expected) {
    await invoke(name, { work_order: "WO-1", idempotency_key: `ui-${++key}`, ...extras }, expected);
  }

  await order("start_operation", { operation: "assembly" }, "OPERATION_OUT_OF_SEQUENCE");
  await order("release_order");
  await order("dispatch_operation", { operation: "machining" });
  await order("issue_material", { batch: "LOT-1" });
  await order("start_operation", { operation: "machining" });
  await order("report_operation", { operation: "machining" }, "WO-1-10");
  await order("dispatch_operation", { operation: "assembly" });
  await order("start_operation", { operation: "assembly" });
  await order("report_operation", { operation: "assembly" });
  await order("dispatch_operation", { operation: "inspection" });
  await order("inspect_units", { result: "fail" });
  await order("close_order", {}, "QUALITY_HOLD");
  await order("disposition", { result: "pass", reason: "复检合格" });
  await order("close_order", {}, '"phase": "closed"');
  await invoke("trace", { subject: "LOT-1" }, "WO-1-10");
  await invoke("trace", { subject: "WO-1-01" }, "LOT-1");
  await invoke("mes_status", { work_order: "WO-1" }, '"on_hand": 0');
  await button("关闭运行").click();
  await expect(status).toContainText('"closed": true');
});
