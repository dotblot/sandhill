import { createEffect, createSignal, For, on, onCleanup, Show, untrack } from "solid-js";
import type { Package, PackageUpdate, PackageUpdates, Packages, SessionInfo } from "../backend/types";
import { filterPackages, type HelpRequest, installCode, packageNames, packageStatus, packageTopic, removeCode } from "../model/help";
import { languageLabel } from "../model/machine";
import type { Workspace } from "../model/workspace";
import { ask } from "./dialogs";
import { Icon } from "./icons";
import { SessionPicker } from "./KernelBar";

/** How long after a run finishes the list is read again. */
const REFRESH_MS = 250;
/** What the daemon says when the session runs other code. */
const BUSY = "the session is busy running code";

const message = (err: unknown) => String(err instanceof Error ? err.message : err).replace(/^(daemon error: )?could not check for updates: /, "");

/**
 * The Packages pane: what's installed for the session's language (R's
 * libraries, or the Python environment), with what's attached or imported
 * first. A click opens a package's help. Installing, updating and removing
 * run in the Console, where their output shows, and the list is read again
 * after. R's updates are checked when asked, as that reaches CRAN and
 * Bioconductor from the session.
 */
export function PackagesPane(props: {
  ws: Workspace;
  session?: SessionInfo;
  sessions: SessionInfo[];
  usedBy?: (session: string) => string[];
  onSession: (session: string) => void;
  /** Opens a package's help. */
  onHelp: (request: HelpRequest) => void;
  /** Runs code in the session's Console. */
  onRun: (session: string, code: string) => void;
  /** False while the pane is hidden: nothing is read then. */
  shown: boolean;
  refreshKey?: number;
}) {
  const ws = props.ws;
  const [listed, setListed] = createSignal<Packages>();
  const [error, setError] = createSignal<string>();
  const [filter, setFilter] = createSignal("");
  const [installing, setInstalling] = createSignal(false);
  const [names, setNames] = createSignal("");
  const [conda, setConda] = createSignal(false);
  const [invalid, setInvalid] = createSignal(false);
  /** What checking for updates found, or why it couldn't. */
  const [updates, setUpdates] = createSignal<PackageUpdates | string>();
  const [checking, setChecking] = createSignal(false);
  const id = () => props.session?.id;
  const language = () => props.session?.language.toLowerCase() ?? "";
  const python = () => language().startsWith("python");
  const name = () => (props.session ? languageLabel(props.session.language) : "");
  const busy = () => {
    const session = id();
    return !!session && ws.isBusy(session);
  };

  let generation = 0;
  async function refresh() {
    const session = id();
    if (!session || !props.shown || ws.isBusy(session)) return;
    const mine = ++generation;
    try {
      const read = await ws.call<Packages>("list_packages", { session });
      if (mine !== generation) return;
      setListed(read);
      setError(undefined);
    } catch (err) {
      if (mine === generation && !String(err).includes(BUSY)) setError(String(err));
    }
  }

  createEffect(
    on(id, () => {
      generation++;
      setListed(undefined);
      setError(undefined);
      setConda(false);
      setUpdates(undefined);
    }),
  );

  async function checkUpdates() {
    const session = id();
    if (!session) return;
    setChecking(true);
    try {
      const found = await ws.call<PackageUpdates>("package_updates", { session });
      if (id() === session) setUpdates(found);
    } catch (err) {
      if (id() === session) setUpdates(String(err).includes(BUSY) ? `${name()} is busy; check again once it finishes.` : `Could not check for updates: ${message(err)}`);
    } finally {
      setChecking(false);
    }
  }
  /** Updates for what's installed now, by name: one updated since drops out. */
  const pending = () => {
    const found = updates();
    const now = new Map((listed()?.packages ?? []).map((p) => [p.name, p.version]));
    const out = new Map<string, PackageUpdate>();
    if (typeof found === "object") for (const u of found.updates) if (now.get(u.name) === u.installed && !out.has(u.name)) out.set(u.name, u);
    return out;
  };
  const updatesNote = () => {
    const found = updates();
    if (checking()) return `Checking ${listed()?.bioconductor ? "CRAN and Bioconductor" : "CRAN"} for updates…`;
    if (typeof found === "string") return found;
    const n = pending().size;
    const missed = found?.unreached?.length ? ` Couldn't reach ${found.unreached.join(", ")}.` : "";
    return (n ? `${n} update${n === 1 ? "" : "s"}: ${[...pending().keys()].join(", ")}.` : "Everything is up to date.") + missed;
  };
  function update(names: string[]) {
    const session = id();
    const where = listed();
    if (session && where && names.length) props.onRun(session, installCode(language(), names, where));
  }
  createEffect(() => {
    id();
    props.refreshKey;
    if (props.shown) untrack(() => void refresh());
  });
  let timer: ReturnType<typeof setTimeout> | undefined;
  const unwatch = ws.runs.watchAll((run) => {
    if (run.session !== id() || run.quiet || run.phase !== "done") return;
    clearTimeout(timer);
    timer = setTimeout(() => void refresh(), REFRESH_MS);
  });
  onCleanup(() => {
    unwatch();
    clearTimeout(timer);
  });

  const inUse = (p: Package) => p.attached;
  const shownPackages = () => filterPackages(listed()?.packages ?? [], filter());
  const groups = () => {
    const all = shownPackages();
    return [
      { title: python() ? "Imported" : "Attached", packages: all.filter(inUse) },
      { title: "Installed", packages: all.filter((p) => !inUse(p)) },
    ].filter((g) => g.packages.length);
  };

  const how = () => {
    const where = listed();
    if (python()) return conda() ? "Runs %conda install in the Console, with conda-forge and bioconda" : "Runs %pip install in the Console";
    if (where?.bioconductor) return "Runs BiocManager::install() in the Console: CRAN and Bioconductor";
    return "Runs install.packages() in the Console: CRAN. For Bioconductor, install BiocManager first";
  };

  function install() {
    const session = id();
    const where = listed();
    const picked = packageNames(names(), language());
    if (!picked) return void setInvalid(true);
    if (!session || !where) return;
    props.onRun(session, installCode(language(), picked, where, conda()));
    setNames("");
    setInvalid(false);
    setInstalling(false);
  }

  async function remove(p: Package) {
    const session = id();
    if (!session) return;
    const answer = await ask(
      `Remove ${p.name}?`,
      ["Remove", "Cancel"],
      `${p.name} ${p.version} is removed from ${p.library}. ${python() ? "Restart Python" : "Restart R"} if it is in use.`,
    );
    if (answer === "Remove") props.onRun(session, removeCode(language(), [p]));
  }

  return (
    <div class="vars pkgs">
      <div class="vtool">
        <SessionPicker ws={ws} session={props.session} sessions={props.sessions} usedBy={props.usedBy} onPick={props.onSession} />
        <input
          class="vfilter"
          placeholder="Filter packages"
          aria-label="Filter packages"
          value={filter()}
          onInput={(e) => setFilter(e.currentTarget.value)}
        />
        <Show when={listed()}>
          {(all) => (
            <span class="vmem" title={all().libraries.join("\n")}>
              {all().packages.length.toLocaleString()} installed
            </span>
          )}
        </Show>
        <Show when={!python()}>
          <button
            class="tab-btn"
            title={listed()?.bioconductor ? "Check CRAN and Bioconductor for updates" : "Check CRAN for updates"}
            aria-label="Check for updates"
            disabled={!listed() || checking() || busy()}
            onClick={() => void checkUpdates()}
          >
            <Icon name="fetch" small />
          </button>
        </Show>
        <button class="tab-btn" title="Install packages" aria-pressed={installing()} disabled={!listed()} onClick={() => setInstalling(!installing())}>
          <Icon name="plus" small />
        </button>
      </div>
      <Show when={installing()}>
        <div class="pinstall">
          <div class="pi-row">
            <input
              class="vfilter"
              placeholder={python() ? "Packages, like scanpy leidenalg" : "Packages, like DESeq2 harmony"}
              aria-label="Packages to install"
              value={names()}
              autofocus
              ref={(el) => setTimeout(() => el.focus())}
              onInput={(e) => {
                setNames(e.currentTarget.value);
                setInvalid(false);
              }}
              onKeyDown={(e) => {
                if (e.key === "Enter") install();
                else if (e.key === "Escape") setInstalling(false);
              }}
            />
            <Show when={python() && listed()?.conda}>
              <select class="pi-tool" aria-label="Install with" value={conda() ? "conda" : "pip"} onChange={(e) => setConda(e.currentTarget.value === "conda")}>
                <option value="pip">pip</option>
                <option value="conda">conda</option>
              </select>
            </Show>
            <button class="vbtn primary" disabled={!names().trim() || busy()} title={busy() ? `${name()} is busy` : undefined} onClick={install}>
              Install
            </button>
          </div>
          <div class="pi-note" classList={{ err: invalid() }}>
            {invalid() ? "Type package names, separated by spaces." : how()}
          </div>
        </div>
      </Show>
      <Show when={checking() || updates()}>
        <div class="pupdates" classList={{ err: typeof updates() === "string" }}>
          <span>{updatesNote()}</span>
          <Show when={!checking() && pending().size}>
            <button class="vbtn" disabled={busy()} title="Installs the new versions in the Console" onClick={() => update([...pending().keys()])}>
              Update all
            </button>
          </Show>
          <Show when={!checking()}>
            <button class="tab-btn" title="Hide" aria-label="Hide updates" onClick={() => setUpdates(undefined)}>
              <Icon name="x" small />
            </button>
          </Show>
        </div>
      </Show>
      <div class="vlist">
        <Show when={props.session} fallback={<p class="vempty">Start R or Python in the Console, and its installed packages show here.</p>}>
          <Show when={busy()}>
            <div class="vbusy">{name()} is running code; the list updates when it finishes.</div>
          </Show>
          <Show when={error()}>
            <div class="var-note err">{error()}</div>
          </Show>
          <Show when={listed() && !shownPackages().length}>
            <p class="vempty">No packages match "{filter().trim()}".</p>
          </Show>
          <For each={groups()}>
            {(group) => (
              <>
                <div class="vh">{group.title}</div>
                <For each={group.packages}>
                  {(p) => (
                    <div
                      class="var pkgrow"
                      title={[p.title, p.library].filter(Boolean).join("\n")}
                      onClick={() => props.onHelp(packageTopic(p, language()))}
                    >
                      <Icon name="pkg" small />
                      <span class="nm">{p.name}</span>
                      <span class="sm">
                        {p.version} <em>{p.title}</em>
                      </span>
                      <Show when={pending().get(p.name)} fallback={<span class="sz">{packageStatus(p, language())}</span>}>
                        {(u) => (
                          <button
                            class="sz upd"
                            title={`Update ${p.name} to ${u().available}${u().bioconductor ? " from Bioconductor" : ""}, in the Console. Restart R afterwards if it is loaded.`}
                            disabled={busy()}
                            onClick={(e) => {
                              e.stopPropagation();
                              update([p.name]);
                            }}
                          >
                            update {u().available}
                          </button>
                        )}
                      </Show>
                      <Show when={!p.base} fallback={<span />}>
                        <button
                          class="vw prm"
                          title={`Remove ${p.name}`}
                          aria-label={`Remove ${p.name}`}
                          disabled={busy()}
                          onClick={(e) => {
                            e.stopPropagation();
                            void remove(p);
                          }}
                        >
                          <Icon name="trash" small />
                        </button>
                      </Show>
                    </div>
                  )}
                </For>
              </>
            )}
          </For>
        </Show>
      </div>
    </div>
  );
}
