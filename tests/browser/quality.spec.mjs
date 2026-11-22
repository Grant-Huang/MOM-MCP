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

test("P8D-02 Record measurements and complete inspection", async ({ page }) => {
  await page.goto("/scenarios");
  const button = (name) => page.getByRole("button", { name, exact: true });
  const status = page.getByRole("status");
  await button("加载场景").click();
  await page.getByLabel("已发布场景").selectOption("assembly@16");
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

  await invoke("create_inspection", {
    inspection_id: "QC-1", work_order: "WO-E", specification_version: "QA-DIM@2",
    expected_cursor: 0, idempotency_key: "inspection",
  }, '"status": "pending"');
  const measurement = await invoke("record_measurement", {
    inspection_id: "QC-1", characteristic: "width", value: 4.6, unit: "mm",
    lower_limit: 4.0, upper_limit: 4.5, expected_cursor: 1, idempotency_key: "width",
  }, '"passed": false');
  await invoke("complete_inspection", {
    inspection_id: "QC-1", expected_cursor: 2, idempotency_key: "complete",
  }, '"status": "failed"');
  const found = await invoke("get_inspection", { inspection_id: "QC-1" }, '"measurements"');
  expect(found.inspection.status).toBe("failed");
  expect(found.measurements).toEqual([measurement.measurement]);
});

test("P8D-03 Hold disposition and release", async ({ page }) => {
  await page.goto("/scenarios");
  const button = (name) => page.getByRole("button", { name, exact: true });
  const status = page.getByRole("status");
  await button("加载场景").click();
  await page.getByLabel("已发布场景").selectOption("cnc@18");
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

  const inspection = { inspection_id: "QC-1" };
  const write = (key, cursor, extra = {}) => ({
    ...inspection, ...extra, idempotency_key: key, expected_cursor: cursor,
  });
  const audit = { responsible: "QA-01", evidence: "REVIEW-1", reason: "dimensional review" };
  await invoke("create_inspection", write("create", 0, {
    work_order: "WO-1", specification_version: "QA-DIM@2",
  }), '"status": "pending"');
  await invoke("record_measurement", write("measure", 1, {
    characteristic: "width", unit: "mm", value: 4.8, lower_limit: 4.0, upper_limit: 4.5,
  }), '"passed": false');
  await invoke("complete_inspection", write("finish", 2), '"status": "failed"');
  await invoke("open_nonconformance", write("open", 3, audit), '"status": "open"');
  await invoke("place_quality_hold", write("hold", 4, audit), '"status": "held"');
  await invoke("close_work_order", {
    work_order: "WO-1", idempotency_key: "close", expected_cursor: 5,
  }, "QUALITY_HOLD");
  await invoke("release_quality_hold", write("release", 5, audit), "NONCONFORMANCE_STATE_INVALID");
  await invoke("disposition_nonconformance", write("dispose", 5, { ...audit, decision: "scrap" }), '"status": "disposed"');
  await invoke("release_quality_hold", write("release", 6, audit), '"status": "released"');
  await invoke("close_work_order", {
    work_order: "WO-1", idempotency_key: "close", expected_cursor: 7,
  }, "QUALITY_SCRAPPED");
  const detail = await invoke("get_inspection", inspection, '"status": "released"');
  expect(detail.nonconformance.history.map(({ action }) => action)).toEqual([
    "open_nonconformance", "place_quality_hold", "disposition_nonconformance", "release_quality_hold",
  ]);
});

test("P9A-UI Activate a versioned inspection plan", async ({ page }) => {
  await page.goto("/scenarios");
  const button = (name) => page.getByRole("button", { name, exact: true });
  const status = page.getByRole("status");
  await button("加载场景").click();
  await page.getByLabel("已发布场景").selectOption("cnc@19");
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

  const characteristics = { width: { unit: "mm", lower_limit: 4, upper_limit: 5 } };
  const plan = { plan_id: "DIM", revision: 1 };
  const inspect = { inspection_id: "QC-1", work_order: "WO-1", specification_version: "DIM@1" };
  await invoke("create_inspection_plan_revision", {
    ...plan, characteristics, expected_cursor: 0, idempotency_key: "draft",
  }, '"status": "draft"');
  await invoke("create_inspection", { ...inspect, expected_cursor: 1, idempotency_key: "create" }, "PLAN_REVISION_NOT_ACTIVE");
  await invoke("activate_inspection_plan_revision", {
    ...plan, expected_cursor: 1, idempotency_key: "activate",
  }, '"status": "active"');
  await invoke("create_inspection", { ...inspect, expected_cursor: 2, idempotency_key: "create" }, '"status": "pending"');
  const found = await invoke("get_inspection", { inspection_id: "QC-1" }, '"inspection_plan"');
  expect(found.inspection_plan.characteristics).toEqual(characteristics);
  await invoke("record_measurement", {
    inspection_id: "QC-1", characteristic: "width", unit: "mm", value: 5.1,
    lower_limit: 4, upper_limit: 6, expected_cursor: 3, idempotency_key: "measurement",
  }, "MEASUREMENT_SPECIFICATION_MISMATCH");
});
