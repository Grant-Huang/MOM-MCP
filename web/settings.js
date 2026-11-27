const result = document.getElementById("result");
const draft = document.getElementById("draft");
async function invoke(name, arguments_) {
  const response = await fetch("/api/settings", {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ name, arguments: arguments_ }),
  });
  const value = await response.json();
  result.textContent = JSON.stringify(value, null, 2);
}
function on(id, handler) {
  document.getElementById(id).onclick = async () => {
    try {
      await handler();
    } catch (error) {
      result.textContent = error instanceof SyntaxError ? "草稿参数必须是有效的 JSON" : "设置请求失败";
    }
  };
}
on("read", () => invoke("get_mes_settings", {}));
on("create", () => invoke("create_mes_settings_draft", JSON.parse(draft.value)));
on("validate", () => invoke("validate_mes_settings_draft", { draft_id: JSON.parse(draft.value).draft_id }));
on("preview", () => invoke("preview_mes_settings_draft", { draft_id: JSON.parse(draft.value).draft_id }));
