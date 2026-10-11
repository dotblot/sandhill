// Before projects, the app showed one folder and remembered its tabs in the
// web view's storage. These bring that folder back as a project once.

import type { ProjectState } from "../backend/types";

const ROOT_KEY = "sandhill.root";
const TABS_KEY = "sandhill.tabs";

/** The folder the app showed before projects, if any. */
export function previousFolder(): string | undefined {
  try {
    return localStorage.getItem(ROOT_KEY) || undefined;
  } catch {
    return undefined;
  }
}

/**
 * The tabs remembered for `project` if it is that folder, forgotten once
 * read: from then on the daemon keeps the project's tabs.
 */
export function takeLegacyState(project: string): ProjectState {
  try {
    if (localStorage.getItem(ROOT_KEY) !== project) return {};
    const tabs: unknown = JSON.parse(localStorage.getItem(TABS_KEY) ?? "[]");
    localStorage.removeItem(ROOT_KEY);
    localStorage.removeItem(TABS_KEY);
    return { tabs: Array.isArray(tabs) ? tabs.filter((t) => typeof t === "string") : [] };
  } catch {
    return {};
  }
}
