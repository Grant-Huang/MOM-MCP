import { expect, test } from "@playwright/test";

test("P16-UI Read site settings outside the production gateway", async ({ page }) => {
  await page.goto("/settings");
  await page.getByRole("button", { name: "读取工作中心与班次" }).click();
  await expect(page.getByRole("status")).toContainText('"site_id": "SITE-A"');
  const settings = JSON.parse(await page.getByRole("status").textContent());
  expect(settings.active_version).toBe(1);
  expect(settings.shifts[0].work_center).toBe("MACH-01");
  const request = { data: { name: "get_mes_settings", arguments: {} } };
  const denied = await page.request.post("/api/scenarios", request);
  expect(denied.status()).toBe(400);
  expect((await denied.json()).code).toBe("UNKNOWN_TOOL");
  request.data.arguments = { site_id: "SITE-B" };
  const forged = await page.request.post("/api/settings", request);
  expect((await forged.json()).code).toBe("INVALID_ARGUMENT");
});

test("P16-03 Preview a night shift without activating it", async ({ page }) => {
  await page.goto("/settings");
  const status = page.getByRole("status");
  await page.getByLabel("草稿参数").fill(JSON.stringify({
    draft_id: "NIGHT", base_version: 1, idempotency_key: "night",
    work_centers: [{ work_center: "MACH-01", active: true }],
    shifts: [{ shift_id: "NIGHT", work_center: "MACH-01", start_local: "22:00", end_local: "06:00" }],
  }));
  await page.getByRole("button", { name: "保存草稿" }).click();
  await expect(status).toContainText('"draft_id": "NIGHT"');
  await page.getByRole("button", { name: "校验草稿" }).click();
  await expect(status).toContainText('"valid": true');
  await page.getByRole("button", { name: "预览影响" }).click();
  await expect(status).toContainText('"existing_runs_affected": false');
  await expect(status).toContainText('"start_local": "22:00"');
  await page.getByRole("button", { name: "读取工作中心与班次" }).click();
  await expect(status).toContainText('"start_local": "08:00"');
  await expect(status).toContainText('"active_version": 1');
});
