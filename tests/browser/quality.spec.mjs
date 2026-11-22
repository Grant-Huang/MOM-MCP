import { test, expect } from "@playwright/test";

test("P8D-UI Public inspection registry", async ({ page }) => {
  await page.goto("/scenarios");
  const button = (name) => page.getByRole("button", { name, exact: true });
  const status = page.getByRole("status");
  await button("加载场景").click();
  await page.getByLabel("已发布场景").selectOption("cnc@16");
  await button("打开场景").click();
  await expect(status).toContainText('"run_id"');
  await button("搜索工具").click();

  async function invoke(name, args, expected) {
    await page.getByLabel("匹配工具").selectOption(name);
    await page.getByLabel("JSON 参数").fill(JSON.stringify(args));
    await button("调用工具").click();
    await expect(status).toContainText(expected);
    return JSON.parse(await status.textContent());
  }

  const created = await invoke("create_inspection", {
    inspection_id: "QC-1",
    work_order: "WO-1",
    specification_version: "QA-DIM@2",
    expected_cursor: 0,
    idempotency_key: "inspection",
  }, '"status": "pending"');
  const listed = await invoke("list_inspections", { work_order: "WO-1" }, '"inspections"');
  expect(listed.inspections).toEqual([created.inspection]);
  const found = await invoke("get_inspection", { inspection_id: "QC-1" }, '"specification_version": "QA-DIM@2"');
  expect(found.inspection).toEqual(created.inspection);
});
