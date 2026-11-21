import { test, expect } from "@playwright/test";

test("P6-04 keyboard navigation on desktop and mobile", async ({ page }) => {
  for (const viewport of [{ width: 1280, height: 800 }, { width: 375, height: 740 }]) {
    await page.setViewportSize(viewport);
    await page.goto("/");
    const instructions = page.getByRole("link", { name: "使用说明", exact: true });
    let reached = false;
    for (let press = 0; press < 8; press++) {
      await page.keyboard.press("Tab");
      if (await instructions.evaluate((link) => document.activeElement === link)) {
        reached = true;
        break;
      }
    }
    expect(reached).toBe(true);
    await page.keyboard.press("Enter");
    await expect(page).toHaveURL(/#usage$/);
    await expect(page.getByRole("heading", { name: "服务端共享仿真时钟" })).toBeVisible();
    await expect(page.getByRole("heading", { name: "五行业场景" })).toBeVisible();
    await expect(page.locator("pre code")).toContainText("run_id");
    expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBeLessThanOrEqual(viewport.width);
    const deployment = await page.request.get("/docs/RELEASE.md");
    expect(deployment.ok()).toBe(true);
    expect(await deployment.text()).toContain("cargo build --release --locked");
    await page.getByRole("link", { name: "AgentNexus stdio 配置" }).click();
    await expect(page.locator("body")).toContainText("--scenario-tools");
  }
});

test("P6-UI three production scenarios share one browser session", async ({ page }) => {
  await page.goto("/");
  await page.getByRole("link", { name: "打开场景验收台" }).click();
  const response = page.getByRole("status");
  const button = (name) => page.getByRole("button", { name, exact: true });
  const selection = page.getByLabel("已发布场景");
  const active = page.getByLabel("当前运行");
  const tool = page.getByLabel("匹配工具");
  await button("加载场景").click();
  await expect(response).toContainText("终测与交付");
  const ids = new Map();

  async function call(name, args, expected) {
    await page.getByLabel("搜索词").fill(name);
    await button("搜索工具").click();
    await expect(tool).toHaveValue(name);
    await page.getByLabel("JSON 参数").fill(JSON.stringify(args));
    await button("调用工具").click();
    await expect(response).toContainText(expected);
  }

  for (const scenario of ["cnc@2", "smt@2", "assembly@2"]) {
    await selection.selectOption(scenario);
    await button("打开场景").click();
    await expect(response).toContainText('"run_id"');
    ids.set(scenario, await active.inputValue());
  }
  expect(new Set(ids.values()).size).toBe(3);
  await call("assembly_priority", { work_order: "WO-E", priority: 5, idempotency_key: "cross-industry" }, '"priority": 5');
  await active.selectOption(ids.get("cnc@2"));
  await call("hold_material", { batch: "LOT-1", reason: "隔离", idempotency_key: "cross-industry" }, '"held": true');
  await active.selectOption(ids.get("smt@2"));
  await call("smt_release", { work_order: "WO-A", idempotency_key: "cross-industry" }, '"ok": true');
  await call("smt_status", { work_order: "WO-A" }, '"released": true');
  await active.selectOption(ids.get("cnc@2"));
  await call("mes_status", { work_order: "WO-1" }, "QUALITY_HOLD");
  await active.selectOption(ids.get("assembly@2"));
  await call("assembly_kit", { work_order: "WO-E" }, '"ready": true');
  for (const id of ids.values()) {
    await active.selectOption(id);
    await button("查看运行").click();
    await expect(response).toContainText('"cursor": 1');
    await button("关闭运行").click();
    await expect(response).toContainText('"closed": true');
  }
});
