import { test, expect } from "@playwright/test";

test("P2-UI shared clock and reproducible delivered events", async ({ page }) => {
  await page.goto("/scenarios");
  const status = page.getByRole("status");
  const button = (name) => page.getByRole("button", { name, exact: true });
  const active = page.getByLabel("当前运行");
  await button("加载场景").click();
  await expect(status).toContainText("model_version");
  await page.getByLabel("已发布场景").selectOption("cnc@1");
  await page.getByLabel("随机种子").fill("42");
  async function open() {
    await button("打开场景").click();
    await expect(status).toContainText('"run_id"');
    return active.inputValue();
  }
  async function events() {
    await button("读取事件").click();
    await expect(status).toContainText('"events"');
    return JSON.parse(await status.textContent());
  }
  const first = await open();
  expect((await events()).clock).toBe(0);
  await button("推进一个事件").click();
  await expect(status).toContainText('"event_cursor": 1');
  const baseline = await events();
  expect(baseline.clock).toBeGreaterThan(0);
  expect(baseline.events[0].kind).toBe("cnc_vibration");

  const second = await open();
  expect(second).not.toBe(first);
  expect((await events()).clock).toBe(0);
  await button("推进一个事件").click();
  await expect(status).toContainText('"event_cursor": 1');
  expect(await events()).toEqual(baseline);

  await active.selectOption(first);
  await button("查看运行").click();
  await expect(status).toContainText(`"clock": ${baseline.clock}`);
  await button("关闭运行").click();
  await expect(status).toContainText('"closed": true');
  await active.selectOption(second);
  await button("关闭运行").click();
  await expect(status).toContainText('"closed": true');
});
