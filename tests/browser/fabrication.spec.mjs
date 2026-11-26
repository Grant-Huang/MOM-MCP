import { test, expect } from "@playwright/test";

test("P7-UI sheet and harness exception flows share the scenario console", async ({ page }) => {
  await page.goto("/");
  await expect(page.getByRole("heading", { name: "五行业场景" })).toBeVisible();
  await page.getByRole("link", { name: "打开场景验收台" }).click();
  const status = page.getByRole("status");
  const button = (name) => page.getByRole("button", { name, exact: true });
  const tools = page.getByLabel("匹配工具");
  const selection = page.getByLabel("已发布场景");
  const run = page.getByLabel("当前运行");
  let key = 0;
  await button("加载场景").click();
  await expect(status).toContainText("端子压接与复测");

  async function open(id, tool) {
    await selection.selectOption(id);
    await button("打开场景").click();
    await expect(status).toContainText('"run_id"');
    const value = await run.inputValue();
    await button("搜索工具").click();
    await expect(tools.locator(`option[value="${tool}"]`)).toHaveCount(1);
    return value;
  }
  async function invoke(name, args, expected = '"ok": true') {
    await tools.selectOption(name);
    const payload = name.endsWith("_trace") ? args : { ...args, idempotency_key: `fabrication-${++key}` };
    await page.getByLabel("JSON 参数").fill(JSON.stringify(payload));
    await button("调用工具").click();
    await expect(status).toContainText(expected);
  }
  async function advance(cursor) {
    await button("推进一个事件").click();
    await expect(status).toContainText(`"event_cursor": ${cursor}`);
  }

  const sheet = await open("sheet-metal@1", "sheet_issue");
  await advance(1);
  await advance(2);
  await invoke("sheet_issue", { stock: "SHEET-FULL" }, "STOCK_UNAVAILABLE");
  await invoke("sheet_issue", { stock: "REM-OK" });
  await invoke("sheet_nest", {});
  await invoke("sheet_cut", { machine: "LASER-A" }, "MACHINE_UNAVAILABLE");
  await invoke("sheet_cut", { machine: "LASER-B" });
  await invoke("sheet_form", {});
  await invoke("sheet_inspect", { result: "pass" });
  await invoke("sheet_return", {}, '"phase": "complete"');
  await invoke("sheet_trace", { subject: "LOT-AL-2" }, '"area_mm2": 300');

  const harness = await open("wire-harness@1", "harness_cut");
  expect(harness).not.toBe(sheet);
  await invoke("harness_cut", { serial: "WIRE-01" });
  await invoke("harness_crimp", { serial: "WIRE-01", terminal_batch: "TERM-ALT" }, "QA-TERMINAL-12");
  await advance(1);
  await invoke("harness_sample", { event_cursor: 1 }, "WIRE-01");
  await invoke("harness_release", { serial: "WIRE-01" }, "TEST_PASS_REQUIRED");
  await invoke("harness_rework", { serial: "WIRE-01", terminal_batch: "TERM-STD", reason: "抽检返工" }, "HARNESS-2");
  await invoke("harness_test", { serial: "WIRE-01", result: "pass" }, '"phase": "passed"');
  await invoke("harness_release", { serial: "WIRE-01" }, '"phase": "released"');
  await invoke("harness_trace", { subject: "TERM-ALT" }, "TERM-STD");
  for (const id of [sheet, harness]) {
    await run.selectOption(id);
    await button("关闭运行").click();
    await expect(status).toContainText('"closed": true');
  }
});

test("P13-UI Cut a shared sheet nest", async ({ page }) => {
  await page.goto("/scenarios");
  const button = (name) => page.getByRole("button", { name, exact: true });
  const status = page.getByRole("status");
  await button("加载场景").click();
  await page.getByLabel("已发布场景").selectOption("sheet-metal@2");
  await button("打开场景").click();
  await expect(status).toContainText('"run_id"');
  await button("搜索工具").click();
  const invoke = async (name, args, evidence) => {
    await page.getByLabel("匹配工具").selectOption(name);
    await page.getByLabel("JSON 参数").fill(JSON.stringify({ nest_id: "N-BROWSER", ...args }));
    await button("调用工具").click();
    await expect(status).toContainText(evidence);
    if (name === "get_sheet_nest") await expect(status).not.toContainText('"cursor"');
    return JSON.parse(await status.textContent());
  };
  await invoke("create_sheet_nest", {
    stock: "SHEET-FULL", work_orders: ["WO-SHEET-A", "WO-SHEET-B"], idempotency_key: "plan",
  }, '"status": "planned"');
  await invoke("cut_sheet_nest", { machine: "LASER-B", idempotency_key: "cut" }, '"status": "cut"');
  const { nest } = await invoke("get_sheet_nest", {}, '"remnant_mm2": 500');
  expect(nest.status).toBe("cut");
  expect(nest.lot).toBe("LOT-AL-1");
  expect(nest.order_area_mm2).toEqual({ "WO-SHEET-A": 600, "WO-SHEET-B": 600 });
  expect(nest.input_mm2).toBe(nest.product_mm2 + nest.scrap_mm2 + nest.remnant_mm2);
});

test("P13-04 Route a wide sheet to a compatible laser", async ({ page }) => {
  await page.goto("/scenarios");
  const button = (name) => page.getByRole("button", { name, exact: true });
  const status = page.getByRole("status");
  await button("加载场景").click();
  await page.getByLabel("已发布场景").selectOption("sheet-metal@3");
  await button("打开场景").click();
  await expect(status).toContainText('"run_id"');
  await button("搜索工具").click();
  const invoke = async (name, args, evidence) => {
    await page.getByLabel("匹配工具").selectOption(name);
    await page.getByLabel("JSON 参数").fill(JSON.stringify({ nest_id: "N-WIDE", ...args }));
    await button("调用工具").click();
    await expect(status).toContainText(evidence);
    return JSON.parse(await status.textContent());
  };
  const { nest } = await invoke("create_sheet_nest", {
    stock: "SHEET-FULL", work_orders: ["WO-SHEET-A", "WO-SHEET-B"], idempotency_key: "plan",
  }, '"size_grade": "wide"');
  expect(nest.dimensional_fit.eligible_machines).toEqual(["LASER-B"]);
  await invoke("cut_sheet_nest", { machine: "LASER-A", idempotency_key: "cut" }, "MACHINE_NOT_CAPABLE");
  const cut = await invoke("cut_sheet_nest", { machine: "LASER-B", idempotency_key: "cut" }, '"status": "cut"');
  expect(cut.nest.machine).toBe("LASER-B");
});

test("P13-06 Return and reuse a sheet remnant", ({ page }) => sheetRemnantFlow(page, 4));
test("P13-08 Trace a reused sheet remnant in both directions", ({ page }) => sheetRemnantFlow(page, 5));

test("P13-10 Validate sheet placement and remnant reuse", ({ page }) => sheetRemnantFlow(page, 6));

async function sheetRemnantFlow(page, version) {
  await page.goto("/scenarios");
  const button = (name) => page.getByRole("button", { name, exact: true });
  const status = page.getByRole("status");
  await button("加载场景").click();
  await page.getByLabel("已发布场景").selectOption(`sheet-metal@${version}`);
  await button("打开场景").click();
  await expect(status).toContainText('"run_id"');
  await button("搜索工具").click();
  const invoke = async (name, args, evidence) => {
    await page.getByLabel("匹配工具").selectOption(name);
    await page.getByLabel("JSON 参数").fill(JSON.stringify(args));
    await button("调用工具").click();
    await expect(status).toContainText(evidence);
    return JSON.parse(await status.textContent());
  };
  if (version === 6) {
    await invoke("create_sheet_nest", { nest_id: "FIRST", stock: "SHEET-FULL",
      work_orders: ["WO-SHEET-A", "WO-SHEET-C"], idempotency_key: "plan" }, "NEST_LAYOUT_UNAVAILABLE");
  }
  await invoke("create_sheet_nest", { nest_id: "FIRST", stock: "SHEET-FULL",
    work_orders: ["WO-SHEET-A"], idempotency_key: "plan" }, '"status": "planned"');
  await invoke("cut_sheet_nest", { nest_id: "FIRST", machine: "LASER-B", idempotency_key: "cut" }, '"status": "cut"');
  await invoke("return_sheet_remnant", { nest_id: "FIRST", remnant_id: "REM-REUSE",
    idempotency_key: "return" }, '"returned_remnant": "REM-REUSE"');
  await invoke("create_sheet_nest", { nest_id: "SECOND", stock: "REM-REUSE",
    work_orders: ["WO-SHEET-B"], idempotency_key: "plan-next" }, '"status": "planned"');
  const { nest } = await invoke("cut_sheet_nest", { nest_id: "SECOND", machine: "LASER-A",
    idempotency_key: "cut-next" }, '"status": "cut"');
  expect(nest.parent_stock).toBe("SHEET-FULL");
  expect(nest.lot).toBe("LOT-AL-1");
  expect(nest.input_mm2).toBe(900);
  if (version >= 5) {
    const forward = await invoke("trace_sheet_material", { subject_type: "stock", subject: "SHEET-FULL" }, '"subject_type": "stock"');
    expect(forward.cuts.map(cut => cut.nest_id)).toEqual(["FIRST", "SECOND"]);
    const backward = await invoke("trace_sheet_material", { subject_type: "work_order", subject: "WO-SHEET-B" }, '"subject_type": "work_order"');
    if (version === 6) {
      expect(forward.cuts[0].placements["WO-SHEET-A"]).toEqual({ x_mm: 0, y_mm: 0, length_mm: 30, width_mm: 20 });
      expect(forward.cuts[0].remnant_dimensions).toEqual({ length_mm: 30, width_mm: 30 });
      expect(forward.cuts[1].placements["WO-SHEET-B"]).toEqual({ x_mm: 0, y_mm: 0, length_mm: 30, width_mm: 20 });
    }
    expect(backward.cuts).toEqual(forward.cuts);
  }
}

test("P14-UI Cut wire batches and recover from short stock", async ({ page }) => {
  await page.goto("/scenarios");
  const button = (name) => page.getByRole("button", { name, exact: true });
  const status = page.getByRole("status");
  await button("加载场景").click();
  await page.getByLabel("已发布场景").selectOption("wire-harness@2");
  await button("打开场景").click();
  await expect(status).toContainText('"run_id"');
  await button("搜索工具").click();
  const invoke = async (name, args, evidence) => {
    await page.getByLabel("匹配工具").selectOption(name);
    await page.getByLabel("JSON 参数").fill(JSON.stringify(args));
    await button("调用工具").click();
    await expect(status).toContainText(evidence);
    return JSON.parse(await status.textContent());
  };
  await invoke("cut_harness_batch", { batch_id:"FIRST", wire_lot:"WIRE-CU-1",
    serial_numbers:["WIRE-01","WIRE-02"], idempotency_key:"first" }, '"remaining_mm": 180');
  const second = { batch_id:"SECOND", wire_lot:"WIRE-CU-1", serial_numbers:["WIRE-03"], idempotency_key:"second" };
  await invoke("cut_harness_batch", second, "INSUFFICIENT_WIRE_LENGTH");
  await invoke("cut_harness_batch", { ...second, wire_lot:"WIRE-CU-2" }, '"remaining_mm": 140');
  const { batch } = await invoke("get_harness_cut_batch", { batch_id:"FIRST" }, '"batch_id": "FIRST"');
  expect(batch.wire_lot).toBe("WIRE-CU-1");
  expect(batch.input_mm).toBe(batch.product_mm + batch.trim_mm);
  expect(Object.keys(batch.segments)).toEqual(["WIRE-01","WIRE-02"]);
});

test("P14-04 Check terminal and tooling qualification for a cut endpoint", async ({ page }) => {
  await page.goto("/scenarios");
  const button = (name) => page.getByRole("button", { name, exact: true });
  const status = page.getByRole("status");
  await button("加载场景").click();
  await page.getByLabel("已发布场景").selectOption("wire-harness@3");
  await button("打开场景").click();
  await expect(status).toContainText('"run_id"');
  await button("搜索工具").click();
  const invoke = async (name, args, evidence) => {
    await page.getByLabel("匹配工具").selectOption(name);
    await page.getByLabel("JSON 参数").fill(JSON.stringify(args));
    await button("调用工具").click();
    await expect(status).toContainText(evidence);
    return JSON.parse(await status.textContent());
  };
  await invoke("cut_harness_batch", { batch_id: "CUT-Q", wire_lot: "WIRE-CU-1",
    serial_numbers: ["WIRE-01"], idempotency_key: "cut" }, '"batch_id": "CUT-Q"');
  const args = { serial_number: "WIRE-01", segment_id: "POWER", end: "B",
    terminal_lot: "TERM-PWR-ALT", tooling_id: "TOOL-PWR-EXPIRED" };
  const tool = "check_harness_crimp_qualification";
  await invoke(tool, args, "TOOLING_CALIBRATION_EXPIRED");
  const { qualification } = await invoke(tool, { ...args, tooling_id: "TOOL-PWR-1" }, '"qualified": true');
  expect(qualification.cut_batch_id).toBe("CUT-Q");
  expect(qualification.terminal_qualification).toBe("TERM-PWR-B-ALT@1");
  expect(qualification.tooling_qualification).toBe("TOOL-PWR@1");
});

test("P14-06 Crimp and verify four harness endpoints", async ({ page }) => {
  await page.goto("/scenarios");
  const button = (name) => page.getByRole("button", { name, exact: true });
  const status = page.getByRole("status");
  await button("加载场景").click();
  await page.getByLabel("已发布场景").selectOption("wire-harness@4");
  await button("打开场景").click();
  await expect(status).toContainText('"run_id"');
  await button("搜索工具").click();
  const invoke = async (name, args, evidence) => {
    await page.getByLabel("匹配工具").selectOption(name);
    await page.getByLabel("JSON 参数").fill(JSON.stringify(args));
    await button("调用工具").click();
    await expect(status).toContainText(evidence);
    return JSON.parse(await status.textContent());
  };
  const serial_number = "WIRE-01";
  await invoke("cut_harness_batch", { batch_id:"CUT-Q", wire_lot:"WIRE-CU-1",
    serial_numbers:[serial_number], idempotency_key:"cut" }, '"batch_id": "CUT-Q"');
  const crimp = "crimp_harness_endpoint";
  const endpoint = { serial_number, segment_id:"POWER", end:"B",
    terminal_lot:"TERM-PWR-ALT", tooling_id:"TOOL-PWR-1", idempotency_key:"power-b" };
  await invoke(crimp, { ...endpoint, tooling_id:"TOOL-PWR-EXPIRED" }, "TOOLING_CALIBRATION_EXPIRED");
  await invoke(crimp, endpoint, '"remaining_terminals": 0');
  await invoke("get_harness_crimp_status", { serial_number }, '"complete": false');
  for (const [segment_id, end, terminal_lot, tooling_id, cursor] of [
    ["POWER","A","TERM-PWR-1","TOOL-PWR-1",3],
    ["SIGNAL","A","TERM-SIG-1","TOOL-SIG-1",4],
    ["SIGNAL","B","TERM-SIG-1","TOOL-SIG-1",5],
  ]) {
    await invoke(crimp, { serial_number, segment_id, end, terminal_lot, tooling_id,
      idempotency_key:segment_id+end }, `"cursor": ${cursor}`);
  }
  const result = await invoke("get_harness_crimp_status", { serial_number }, '"complete": true');
  expect(result.crimps).toHaveLength(4);
  expect(result.crimps[0]).toMatchObject({ cut_batch_id:"CUT-Q", terminal_qualification:"TERM-PWR-B-ALT@1" });
});
