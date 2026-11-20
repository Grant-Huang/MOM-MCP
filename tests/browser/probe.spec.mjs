import { test, expect } from "@playwright/test";

test("P0-UI browser discovery, write, rejection and recovery", async ({ page }) => {
  await page.goto("/");
  const document = await page.request.get("docs/FEATURES.md");
  expect(document.status()).toBe(200);
  expect(await document.text()).toContain("服务端共享仿真时钟");
  await page.getByRole("link", { name: "打开接入验收台" }).click();
  await expect(page.getByRole("heading", { name: "接入验收台" })).toBeVisible();
  const status = page.getByRole("status");
  const call = async (expected) => {
    await page.getByRole("button", { name: "调用工具" }).click();
    await expect(status).toContainText(expected);
  };
  await page.getByRole("button", { name: "加载 MCP 工具" }).click();
  await expect(status).toHaveText("已发现 2 个工具");
  await page.getByLabel("工具", { exact: true }).selectOption("get_probe");
  await call('"revision": 0');
  await page.getByLabel("工具", { exact: true }).selectOption("set_probe");
  await page.getByText("工具契约", { exact: true }).click();
  await expect(page.locator("#schema")).toContainText("idempotency_key");
  await page.getByLabel("测试值").fill("101");
  await page.getByLabel("幂等键").fill("browser-write");
  await call("VALUE_OUT_OF_RANGE");
  await page.getByLabel("测试值").fill("42");
  await call('"value": 42');
  await expect(status).toContainText('"revision": 1');
  await call('"revision": 1');
  await page.getByLabel("工具", { exact: true }).selectOption("get_probe");
  await call('"value": 42');
  await expect(status).toContainText('"revision": 1');
});
