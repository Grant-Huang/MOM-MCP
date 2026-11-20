const select = document.querySelector("#tools");
const schema = document.querySelector("#schema");
const output = document.querySelector("#result");
const submit = document.querySelector("#submit");
const fields = document.querySelector("#write-fields");
const discover = document.querySelector("#discover");
let tools = [];

function showTool() {
  const tool = tools.find(({ name }) => name === select.value);
  schema.textContent = tool ? JSON.stringify(tool, null, 2) : "";
  fields.hidden = select.value !== "set_probe";
  submit.disabled = !tool;
}

discover.addEventListener("click", async () => {
  discover.disabled = true;
  try {
    const response = await fetch("/api/tools");
    if (!response.ok) throw new Error("MCP 工具发现失败");
    ({ tools } = await response.json());
    select.replaceChildren(...tools.map(({ name }) => new Option(name, name)));
    select.disabled = !tools.length;
    showTool();
    output.textContent = `已发现 ${tools.length} 个工具`;
  } catch (error) {
    output.textContent = error.message;
  } finally {
    discover.disabled = false;
  }
});

select.addEventListener("change", showTool);
document.querySelector("#call").addEventListener("submit", async (event) => {
  event.preventDefault();
  const value = document.querySelector("#value").valueAsNumber;
  const args = select.value === "set_probe"
    ? { value: Number.isNaN(value) ? null : value, idempotency_key: document.querySelector("#key").value }
    : {};
  submit.disabled = true;
  output.textContent = "正在调用";
  try {
    const response = await fetch("/api/call", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ name: select.value, arguments: args }),
    });
    output.textContent = JSON.stringify(await response.json(), null, 2);
  } catch {
    output.textContent = "无法读取调用结果，请先查询当前状态";
  } finally {
    showTool();
  }
});
