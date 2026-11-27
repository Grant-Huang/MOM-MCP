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
