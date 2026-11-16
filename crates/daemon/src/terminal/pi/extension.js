// Sandhill's extension for pi, loaded with `pi -e` by sandhilld's pi bridge
// for one run; written into the conversation's terminal folder, never into
// pi's own configuration.
//
// - SANDHILL_MCP_URL and SANDHILL_MCP_TOKEN: Sandhill's MCP server (the
//   user's live R and Python sessions, documents and plots), registered for
//   this session only. The token is taken out of the environment, so the
//   commands pi runs don't inherit it.
// - SANDHILL_TRUSTED_FOLDER: the folder the user trusted in Sandhill that
//   this run starts in. pi's own question about the project's `.pi` files
//   is then answered yes for this run only; nothing is saved in pi's trust.
import { realpathSync } from "node:fs";
import { sep } from "node:path";

const config = (globalThis.__sandhill ??= {
  url: process.env.SANDHILL_MCP_URL,
  token: process.env.SANDHILL_MCP_TOKEN,
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
}
