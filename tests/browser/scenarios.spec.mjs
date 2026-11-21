import { test, expect } from "@playwright/test";

test("P1-UI search, call and verify isolated scenario runs", async ({ page }) => {
  await page.goto("/");
  await page.getByRole("link", { name: "打开场景验收台" }).click();
  const status = page.getByRole("status");
  await page.getByRole("button", { name: "加载场景" }).click();
  await expect(status).toContainText("电子装联 SMT");
  await expect(status).toContainText("工业设备按单装配");

  const scenario = page.getByLabel("已发布场景");
  const active = page.getByLabel("当前运行");
  async function open(id) {
    await scenario.selectOption(id);
    await page.getByRole("button", { name: "打开场景" }).click();
    await expect(status).toContainText('"run_id"');
    return active.inputValue();
  }
  async function find(text) {
    await page.getByLabel("搜索词").fill(text);
    await page.getByRole("button", { name: "搜索工具" }).click();
    await expect(status).toContainText('"tools"');
  }
  async function invoke(payload, expected) {
    await page.getByLabel("JSON 参数").fill(JSON.stringify(payload));
    await page.getByRole("button", { name: "调用工具" }).click();
    await expect(status).toContainText(expected);
  }

  const cnc = await open("cnc@1");
  await find("质量冻结");
  await expect(page.getByLabel("匹配工具")).toHaveValue("hold_material");
  await page.getByRole("button", { name: "查看工具契约" }).click();
  await expect(page.getByLabel("工具契约")).toContainText("idempotency_key");
  await invoke({}, "INVALID_ARGUMENT");
  await invoke({ batch: "LOT-1", reason: "复检", idempotency_key: "ui-first" }, '"held": true');

  const smt = await open("smt@1");
  expect(smt).not.toBe(cnc);
  await find("quality hold");
  await expect(page.getByLabel("匹配工具").locator("option")).toHaveCount(0);
  await find("物料");
  await invoke({ batch: "LOT-1" }, '"held": false');

  await active.selectOption(cnc);
  await find("物料");
  await invoke({ batch: "LOT-1" }, '"held": true');
  await page.getByRole("button", { name: "关闭运行" }).click();
  await expect(status).toContainText('"closed": true');
  await expect(page.getByRole("button", { name: "搜索工具" })).toBeDisabled();

  await active.selectOption(smt);
  await page.getByRole("button", { name: "查看运行" }).click();
  await expect(status).toContainText('"closed": false');
  await expect(status).toContainText('"cursor": 0');
  await page.getByRole("button", { name: "关闭运行" }).click();
  await expect(status).toContainText('"closed": true');
});
