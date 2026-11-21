import { test, expect } from "@playwright/test";

test("P8B-UI Public order discovery and readiness", async ({ page }) => {
  await page.goto("/scenarios");
  const button = (name) => page.getByRole("button", { name, exact: true });
  const status = page.getByRole("status");
  await button("加载场景").click();
  await page.getByLabel("已发布场景").selectOption("cnc@3");
  await button("打开场景").click();
  await expect(status).toContainText('"run_id"');
  await button("搜索工具").click();
  const tool = page.getByLabel("匹配工具");

  async function invoke(name, args, expected) {
    await tool.selectOption(name);
    await page.getByLabel("JSON 参数").fill(JSON.stringify(args));
    await button("调用工具").click();
    await expect(status).toContainText(expected);
  }

  await invoke("list_work_orders", { limit: 1 }, '"next_after_work_order": "WO-1"');
  await invoke("get_release_readiness", { work_order: "WO-1" }, '"ready": true');
  await invoke("release_order", { work_order: "WO-1", idempotency_key: "release" }, '"ok": true');
  await invoke("get_release_readiness", { work_order: "WO-1" }, "INVALID_STATE");
});
