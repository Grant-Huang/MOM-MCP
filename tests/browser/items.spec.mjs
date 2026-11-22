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
