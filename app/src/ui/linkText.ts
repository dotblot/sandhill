import { LOCAL_HOST } from "../backend";
import type { Workspace } from "../model/workspace";

/** One line about a window's daemon: where it runs, or why it can't be reached. */
export function linkText(ws: Workspace): string {
  const link = ws.link();
  const info = ws.host();
  switch (link.status) {
    case "connected":
      return info ? `Connected to sandhilld ${info.daemon_version} on ${info.hostname}` : "Connected";
    case "connecting":
      if (link.detail) return link.detail;
      // An error means the link was up, or a try failed: it is reconnecting.
      return `${link.error ? "Reconnecting" : "Connecting"} to ${ws.backend.host === LOCAL_HOST ? "sandhilld" : ws.backend.host}…`;
    default:
      if (link.stopped) return "Disconnected";
      return `Disconnected${link.error ? `: ${link.error}` : ""}. Retrying…`;
  }
}
