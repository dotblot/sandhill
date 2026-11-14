// Sandhill's tools for Oh My Pi's command line. omp reads MCP servers only
// from its mcp.json files, so this per-run extension (`omp -e`) offers the
// tools of the Sandhill bridge's loopback MCP server instead; the bridge
// passes its address and bearer token in the environment.
const url = process.env.SANDHILL_MCP_URL;
const token = process.env.SANDHILL_MCP_TOKEN;
let next = 0;

async function rpc(method, params) {
  const response = await fetch(url, {
    method: "POST",
    headers: {
      "Content-Type": "application/json",
      Accept: "application/json, text/event-stream",
      Authorization: `Bearer ${token}`,
    },
    body: JSON.stringify({ jsonrpc: "2.0", id: ++next, method, params }),
  });
  if (!response.ok) throw new Error(`Sandhill's tools answered HTTP ${response.status}`);
  const reply = await response.json();
  if (reply.error) throw new Error(reply.error.message ?? "Sandhill's tools failed");
  return reply.result;
}

export default async function sandhill(pi) {
  if (!url || !token) return;
  await rpc("initialize", {
    protocolVersion: "2025-06-18",
    capabilities: {},
    clientInfo: { name: "sandhill-omp", version: "1" },
  });
  const { tools = [] } = await rpc("tools/list", {});
  for (const tool of tools) {
    const schema = tool.inputSchema ?? { type: "object", properties: {} };
    pi.registerTool({
      name: `sandhill_${tool.name}`,
      label: tool.title ?? tool.name,
      description: tool.description ?? "",
      // Offered beside omp's own tools, not behind its tool discovery.
      loadMode: "essential",
      parameters: schema,
      async execute(_id, params) {
        const args = { ...(params ?? {}) };
        // omp adds an intent argument to every tool; Sandhill's take none.
        if (!schema.properties?.i) delete args.i;
        const result = await rpc("tools/call", { name: tool.name, arguments: args });
        const content = (result.content ?? []).map((part) =>
          part.type === "text" || part.type === "image" ? part : { type: "text", text: JSON.stringify(part) },
        );
        if (result.isError) {
          throw new Error(content.map((part) => part.text ?? "").join("\n") || "Sandhill's tool failed");
        }
        return { content, details: result.structuredContent ?? {} };
      },
    });
  }
}
