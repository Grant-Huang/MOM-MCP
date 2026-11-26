// Run only against an authorized Carbon test company and synthetic work orders.
import { pathToFileURL } from "node:url";

const operation = "production_getJob";
const protocol = "2025-11-25";

export async function checkCarbon({ url, token, visibleId, hiddenId, request = fetch }) {
  const endpoint = new URL(url);
  if (endpoint.protocol !== "https:" || endpoint.username || endpoint.password) {
    throw new Error("Use a credential-free HTTPS MCP endpoint URL");
  }
  if (!token || !visibleId || !hiddenId || visibleId === hiddenId) {
    throw new Error("Provide a token and two distinct prearranged fixture IDs");
  }

  const headers = {
    Authorization: `Bearer ${token}`,
    Accept: "application/json, text/event-stream",
    "Content-Type": "application/json",
  };
  let id = 0;
  async function rpc(method, params) {
    const notification = method === "notifications/initialized";
    const requestId = notification ? undefined : ++id;
    const result = await request(endpoint, {
      method: "POST",
      headers,
      body: JSON.stringify({ jsonrpc: "2.0", id: requestId, method, params }),
      redirect: "error",
      signal: AbortSignal.timeout(10_000),
    });
    if (!result.ok) {
      throw new Error(`Carbon ${method} returned HTTP ${result.status}`);
    }
    if (result.headers.has("mcp-session-id")) {
      throw new Error("Carbon returned an unsupported MCP session");
    }
    if (notification) {
      if (result.status !== 202) throw new Error("Carbon did not accept the initialized notification");
      return;
    }
    if (!result.headers.get("content-type")?.includes("application/json")) {
      throw new Error("Carbon did not return the expected legacy JSON envelope");
    }
    const message = await result.json();
    if (message.jsonrpc !== "2.0" || message.id !== requestId || message.error || !message.result) {
      throw new Error(`Carbon ${method} did not return an RPC result`);
    }
    return message.result;
  }

  const initialized = await rpc("initialize", {
    protocolVersion: protocol,
    clientInfo: { name: "mom-carbon-read-smoke", version: "1" },
    capabilities: {},
  });
  if (initialized.protocolVersion !== protocol) {
    throw new Error("Carbon did not negotiate the pinned legacy protocol");
  }
  const tools = initialized.capabilities?.tools;
  if (!tools || typeof tools !== "object" || Array.isArray(tools)) {
    throw new Error("Carbon did not advertise MCP tools");
  }
  headers["MCP-Protocol-Version"] = protocol;
  await rpc("notifications/initialized");

  const description = await rpc("tools/call", {
    name: "describe_tool",
    arguments: { name: operation },
  });
  if (description.isError || !description.content?.some((block) =>
    block.type === "text" && typeof block.text === "string" && block.text.includes(operation))) {
    throw new Error("Carbon did not describe the expected production operation");
  }

  const get = (fixtureId) => rpc("tools/call", {
    name: "call_tool",
    arguments: {
      name: operation,
      arguments: { id: fixtureId },
      fields: ["id", "jobId", "status", "updatedAt"],
    },
  });
  const visible = await get(visibleId);
  if (visible.isError || visible.content?.length !== 1 || visible.content[0].type !== "text") {
    throw new Error("Visible fixture did not return a single MCP text record");
  }
  let row;
  try {
    row = JSON.parse(visible.content[0].text);
  } catch {
    throw new Error("Visible fixture text is not a JSON record");
  }
  if (row?.id !== visibleId || typeof row.jobId !== "string" || !row.jobId.trim()
    || typeof row.status !== "string" || !row.status.trim()) {
    throw new Error("Visible fixture fields do not match the candidate contract");
  }

  const hidden = await get(hiddenId);
  if (hidden.isError !== true) {
    throw new Error("Other-company fixture did not return a tool-level refusal");
  }
  return {
    protocol,
    operation,
    visible: { outcome: "record", hasSourceTimestamp: typeof row.updatedAt === "string" },
    otherCompany: { outcome: "tool-error", classification: "requires-review" },
  };
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  try {
    console.log(JSON.stringify(await checkCarbon({
      url: process.env.MOM_CARBON_MCP_URL,
      token: process.env.MOM_CARBON_TOKEN,
      visibleId: process.env.MOM_CARBON_VISIBLE_JOB_ID,
      hiddenId: process.env.MOM_CARBON_OTHER_COMPANY_JOB_ID,
    })));
  } catch (error) {
    // Fetch and JSON errors can include request or response data. Print only
    // messages controlled by the probe.
    const message = error.message;
    console.error(/^(Use a credential-free|Provide a token|Carbon |Visible fixture |Other-company )/.test(message)
      ? message : "Carbon probe failed before sanitized evidence was produced");
    process.exitCode = 1;
  }
}
