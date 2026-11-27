const result = document.getElementById("result");
document.getElementById("read").onclick = async () => {
  try {
    const response = await fetch("/api/settings", {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({ name: "get_mes_settings", arguments: {} }),
    });
    result.textContent = JSON.stringify(await response.json(), null, 2);
  } catch {
    result.textContent = "读取设置失败";
  }
};
