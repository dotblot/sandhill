// What one window has open: a tab per machine, each showing one project.
// Opening another project on a machine switches that machine's tab to it.

import { LOCAL_HOST, type Place, type WindowPlaces } from "../backend";

/** A key for a project on a machine. */
export function placeKey(place: Place): string {
  return `${place.host}\n${place.path}`;
}

/** Shows `place`: its machine's tab, now on that project, or a new tab at the end. */
export function openPlace(window: WindowPlaces, place: Place): WindowPlaces {
  const has = window.tabs.some((t) => t.host === place.host);
  const tabs = has ? window.tabs.map((t) => (t.host === place.host ? place : t)) : [...window.tabs, place];
  return { tabs, current: place.host };
}

/** Closes a machine's tab; if it was shown, the tab before it shows, or else the Start screen. */
export function closeTab(window: WindowPlaces, host: string): WindowPlaces {
  const index = window.tabs.findIndex((t) => t.host === host);
  if (index < 0) return window;
  const tabs = window.tabs.filter((t) => t.host !== host);
  if (window.current !== host) return { tabs, current: window.current };
  return { tabs, current: tabs[Math.max(0, index - 1)]?.host ?? null };
}

/** The project shown, if not the Start screen. */
export function shownPlace(window: WindowPlaces): Place | undefined {
  return window.tabs.find((t) => t.host === window.current);
}

/** A machine's name in a tab: "Local" for this Mac, else its ssh name. */
export function machineName(host: string): string {
  return host === LOCAL_HOST ? "Local" : host;
}

/** A folder as the tabs show it, with the home folder as "~". */
export function shortPath(path: string, home?: string): string {
  if (home && home !== "/" && (path === home || path.startsWith(`${home}/`))) return `~${path.slice(home.length)}`;
  const user = /^(\/Users\/[^/]+|\/home\/[^/]+|\/root)(\/|$)/.exec(path);
  return user ? `~${path.slice(user[1].length)}` : path;
}

/** A folder's last part, its project name. */
export function projectName(path: string): string {
  return path.slice(path.replace(/\/+$/, "").lastIndexOf("/") + 1).replace(/\/+$/, "") || path;
}

/** The window's title: the project shown and its machine, or Sandhill on the Start screen. */
export function windowTitle(window: WindowPlaces): string {
  const shown = shownPlace(window);
  if (!shown) return "Sandhill";
  const name = projectName(shown.path);
  return shown.host === LOCAL_HOST ? name : `${name} — ${shown.host}`;
}
