const $ = (id) => document.getElementById(id);
const scenarios = new Map();
const runs = new Map();
let busy = false;

function show(value) {
  $("response").textContent = JSON.stringify(value, null, 2);
}

function controls() {
  const run = runs.get($("run").value);
  $("load").disabled = busy;
  $("scenario").disabled = busy || !scenarios.size;
  $("open").disabled = busy || !scenarios.size;
  $("seed").disabled = busy;
  $("run").disabled = busy || !runs.size;
  $("read-run").disabled = busy || !run;
  $("close").disabled = $("find").disabled = busy || !run || run.closed;
  $("tool").disabled = $("describe").disabled = $("invoke").disabled =
    busy || !run || run.closed || !$("tool").options.length;
}

function clearSearch() {
  $("tool").replaceChildren();
  $("contract").textContent = "";
  $("arguments").value = "{}";
}

async function request(name, args = {}) {
  const response = await fetch("/api/scenarios", {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ name, arguments: args }),
  });
  return response.json();
}

async function action(callback) {
  if (busy) return;
  busy = true;
  $("response").textContent = "正在处理";
  controls();
  try {
    show(await callback());
  } catch {
    show({ ok: false, code: "REQUEST_FAILED" });
  } finally {
    busy = false;
    controls();
  }
}

$("load").onclick = () => action(async () => {
  const result = await request("list_scenarios");
  if (result.ok) {
    scenarios.clear();
    for (const spec of result.scenarios) scenarios.set(`${spec.scenario_id}@${spec.version}`, spec);
    $("scenario").replaceChildren(...Array.from(scenarios, ([id, spec]) => new Option(spec.title, id)));
  }
  return result;
});

$("open").onclick = () => action(async () => {
  const { scenario_id, version, title } = scenarios.get($("scenario").value);
  const seed = $("seed").valueAsNumber;
  if (!Number.isSafeInteger(seed) || seed < 0) return { ok: false, code: "INVALID_SEED" };
  const result = await request("open_scenario", { scenario_id, version, seed });
  if (result.ok) {
    runs.set(result.run_id, result);
    $("run").add(new Option(`${title} · ${result.run_id.slice(0, 8)}`, result.run_id));
    $("run").value = result.run_id;
    clearSearch();
  }
  return result;
});

$("run").onchange = () => {
  clearSearch();
  controls();
};
$("read-run").onclick = () => action(() => request("get_run", { run_id: $("run").value }));
$("close").onclick = () => action(async () => {
  const result = await request("close_run", { run_id: $("run").value });
  if (result.ok) {
    runs.set(result.run_id, result);
    clearSearch();
  }
  return result;
});

$("search").onsubmit = (event) => {
  event.preventDefault();
  action(async () => {
    const args = { run_id: $("run").value, query: $("query").value };
    for (const field of ["domain", "effect"]) if ($(field).value) args[field] = $(field).value;
    const result = await request("search_tools", args);
    clearSearch();
    if (result.ok) $("tool").replaceChildren(...result.tools.map(({ name }) => new Option(name, name)));
    return result;
  });
};

$("tool").onchange = () => { $("contract").textContent = ""; };
$("describe").onclick = () => action(async () => {
  const result = await request("describe_tool", { run_id: $("run").value, name: $("tool").value });
  if (result.ok) $("contract").textContent = JSON.stringify(result.tool, null, 2);
  return result;
});

$("call").onsubmit = (event) => {
  event.preventDefault();
  action(async () => {
    let args;
    try {
      args = JSON.parse($("arguments").value);
      if (!args || Array.isArray(args) || typeof args !== "object") throw new Error();
    } catch {
      return { ok: false, code: "INVALID_JSON_OBJECT" };
    }
    return request("call_tool", { run_id: $("run").value, name: $("tool").value, arguments: args });
  });
};
controls();
