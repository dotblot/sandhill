// Sandhill's extension for pi, loaded with `pi -e` by sandhilld's pi bridge
// for one run; written into the conversation's terminal folder, never into
// pi's own configuration.
//
// - SANDHILL_MCP_URL and SANDHILL_MCP_TOKEN: Sandhill's MCP server (the
//   user's live R and Python sessions, documents and plots), registered for
//   this session only. The token is taken out of the environment, so the
//   commands pi runs don't inherit it.
// - SANDHILL_PI_GATE=1 (chat mode): before each tool call pi asks the bridge
//   through RPC mode's extension UI (an `input` titled "sandhill:tool_call"
//   whose placeholder is the call as JSON); the bridge answers
//   {"allow": true} or {"allow": false, "reason": "..."}, asking the user in
//   Sandhill's chat when the conversation's mode says so.
// - SANDHILL_TRUSTED_FOLDER: the folder the user trusted in Sandhill that
//   this run starts in. pi's own question about the project's `.pi` files
//   is then answered yes for this run only; nothing is saved in pi's trust.
import { realpathSync } from "node:fs";
import { sep } from "node:path";

const config = (globalThis.__sandhill ??= {
  url: process.env.SANDHILL_MCP_URL,
  token: process.env.SANDHILL_MCP_TOKEN,
  gate: process.env.SANDHILL_PI_GATE === "1",
  trusted: process.env.SANDHILL_TRUSTED_FOLDER,
});
delete process.env.SANDHILL_MCP_TOKEN;

export default function (pi) {
  if (config.url && config.token) {
    pi.registerMcpServer("sandhill", {
      url: config.url,
      headers: { Authorization: `Bearer ${config.token}` },
      exposure: "direct",
      timeout: 3600,
      description:
        "Sandhill: the user's live R and Python sessions, variables, documents, plots and console",
    });
  }
  if (config.trusted) {
    pi.on("project_trust", async (event) => {
      const real = (path) => {
        try {
          return realpathSync(path);
        } catch {
          return path;
        }
      };
      const [cwd, folder] = [real(event.cwd), real(config.trusted)];
      const inside = cwd === folder || cwd.startsWith(folder.endsWith(sep) ? folder : folder + sep);
      return { trusted: inside ? "yes" : "undecided" };
    });
  }
  if (!config.gate) return;
  pi.on("tool_call", async (event, ctx) => {
    if (ctx.mode !== "rpc") return;
    const call = JSON.stringify({
      toolCallId: event.toolCallId,
      toolName: event.toolName,
      input: event.input,
    });
    const answer = await ctx.ui.input("sandhill:tool_call", call, { signal: ctx.signal });
    let decision;
    try {
      decision = JSON.parse(answer ?? "null");
    } catch {
      decision = null;
    }
    if (decision?.allow === true) return;
    return {
      block: true,
      reason: decision?.reason || "The user did not allow this step in Sandhill.",
    };
  });
}
