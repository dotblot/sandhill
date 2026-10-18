import DOMPurify from "dompurify";
import { createEffect, createSignal, For, type JSX, on, onCleanup, Show, untrack } from "solid-js";
import type { HelpMember, HelpPage, HelpSource, HelpTopics, SessionInfo } from "../backend/types";
import { highlightedLines, languageFor } from "../editor/setup";
import { consoleHelp, docBlocks, exampleCode, type HelpRequest, helpLink, inlineParts, signatureLines } from "../model/help";
import { languageLabel } from "../model/machine";
import type { Workspace } from "../model/workspace";
import { Icon } from "./icons";
import { SessionPicker } from "./KernelBar";

/** How long typing pauses before the help is searched. */
const WAIT_MS = 150;
/** Topics a search reads, and how many show before "more". */
const LIMIT = 50;
const FEW = 6;
/** What the daemon says when the session runs other code. */
const BUSY = "the session is busy running code";

interface Place {
  topic: string;
  package?: string;
}

const message = (err: unknown) => String(err instanceof Error ? err.message : err).replace(/^(daemon error: )?could not (search|read) the (help|source): /, "");

/**
 * The Help pane: search the session's help topics (R's installed packages,
 * or Python's imported modules) and read a page, with its links followed
 * here, Back and Forward, and the function's source a click away. Help runs
 * in the session, quietly, so it waits while the session runs other code.
 */
export function HelpPane(props: {
  ws: Workspace;
  session?: SessionInfo;
  sessions: SessionInfo[];
  usedBy?: (session: string) => string[];
  onSession: (session: string) => void;
  /** A topic to show, from the Console or the Packages pane; `n` changes each time. */
  request?: HelpRequest & { n: number };
  /** Runs code in the session's Console: a page's examples. */
  onRun: (session: string, code: string) => void;
  openLink: (url: string) => void;
  /** False while the pane is hidden: nothing is read then. */
  shown: boolean;
  /** Reads the search and the page again when this changes. */
  refreshKey?: number;
}) {
  const ws = props.ws;
  const [query, setQuery] = createSignal("");
  const [pkg, setPkg] = createSignal("");
  const [found, setFound] = createSignal<HelpTopics>();
  const [searchError, setSearchError] = createSignal<string>();
  const [all, setAll] = createSignal(false);
  const [page, setPage] = createSignal<HelpPage>();
  const [pageError, setPageError] = createSignal<string>();
  const [loading, setLoading] = createSignal(false);
  const [source, setSource] = createSignal<HelpSource | string>();
  const [showSource, setShowSource] = createSignal(false);
  /** Pages visited, for Back and Forward. */
  const [trail, setTrail] = createSignal<Place[]>([]);
  const [at, setAt] = createSignal(-1);
  /** A read that waits for the session to finish running code. */
  const [waiting, setWaiting] = createSignal(false);
  const id = () => props.session?.id;
  const language = () => props.session?.language.toLowerCase() ?? "";
  const python = () => language().startsWith("python");
  const name = () => (props.session ? languageLabel(props.session.language) : "");

  let searchGeneration = 0;
  let pageGeneration = 0;
  let timer: ReturnType<typeof setTimeout> | undefined;

  async function search() {
    const session = id();
    const q = query().trim();
    const p = pkg().trim();
    const mine = ++searchGeneration;
    if (!session || (!q && !p)) {
      setFound(undefined);
      setSearchError(undefined);
      return;
    }
    if (!props.shown) return;
    try {
      const result = await ws.call<HelpTopics>("help_topics", { session, query: q, package: p || undefined, limit: LIMIT });
      if (mine !== searchGeneration) return;
      setFound(result);
      setSearchError(undefined);
    } catch (err) {
      if (mine !== searchGeneration) return;
      if (String(err).includes(BUSY)) setWaiting(true);
      else setSearchError(message(err));
    }
  }

  const searchSoon = () => {
    clearTimeout(timer);
    setAll(false);
    timer = setTimeout(() => void search(), WAIT_MS);
  };

  /**
   * Shows a topic's page; `step` moves along the trail rather than adding
   * to it. A `quiet` try that fails leaves the pane as it was.
   */
  async function open(place: Place, step?: number, quiet = false): Promise<boolean> {
    const session = id();
    if (!session) return false;
    const mine = ++pageGeneration;
    setLoading(true);
    setShowSource(false);
    setSource(undefined);
    try {
      const read = await ws.call<HelpPage>("help_page", { session, topic: place.topic, package: place.package || undefined });
      if (mine !== pageGeneration) return false;
      setPage(read);
      setPageError(undefined);
      const here = { topic: read.topic, package: read.package };
      if (step === undefined) {
        const kept = trail().slice(0, at() + 1);
        setTrail([...kept, here]);
        setAt(kept.length);
      } else {
        setAt(step);
      }
      if (body) body.scrollTop = 0;
      return true;
    } catch (err) {
      if (mine !== pageGeneration) return false;
      if (String(err).includes(BUSY)) {
        pending = place;
        setWaiting(true);
      } else if (!quiet) {
        setPageError(message(err));
        setPage(undefined);
      }
      return false;
    } finally {
      if (mine === pageGeneration) setLoading(false);
    }
  }
  /** A page asked for while the session was busy, read once it's idle. */
  let pending: Place | undefined;

  /** Follows a reference in a Python docstring: as written, then within the page's package. */
  async function follow(ref: string) {
    const current = page();
    if (!current) return;
    const tries = [ref];
    if (!ref.startsWith(`${current.package}.`)) tries.push(`${current.package}.${ref}`);
    // A method of the page's class, by its own name.
    if (current.kind === "class" && !ref.includes(".")) tries.unshift(`${current.topic}.${ref}`);
    for (const topic of tries) if (await open({ topic }, undefined, true)) return;
    setPageError(`No help for ${ref}`);
  }

  async function toggleSource() {
    const current = page();
    const session = id();
    if (showSource()) return void setShowSource(false);
    setShowSource(true);
    if (!current || !session || source()) return;
    try {
      setSource(await ws.call<HelpSource>("help_source", { session, topic: current.topic, package: current.package || undefined }));
    } catch (err) {
      setSource(message(err));
    }
  }

  // A new session starts from scratch.
  createEffect(
    on(id, () => {
      searchGeneration++;
      pageGeneration++;
      setFound(undefined);
      setSearchError(undefined);
      setPage(undefined);
      setPageError(undefined);
      setTrail([]);
      setAt(-1);
      setWaiting(false);
      pending = undefined;
    }),
  );
  createEffect(() => {
    id();
    props.refreshKey;
    if (!props.shown) return;
    untrack(() => {
      void search();
      const current = trail()[at()];
      if (current) void open(current, at());
    });
  });
  // Asked from elsewhere: a topic to open, a package's topics, or a search.
  createEffect(
    on(
      () => props.request,
      (asked) => {
        if (!asked) return;
        if (asked.search) {
          setQuery(asked.topic);
          setPkg(asked.package ?? "");
          return void search();
        }
        if (!asked.topic && asked.package) {
          // A package: its topics, and its overview page if it has one.
          setQuery("");
          setPkg(asked.package);
          void search();
          return void open({ topic: `${asked.package}-package`, package: asked.package }, undefined, true);
        }
        // A topic opened from elsewhere gets the room the matches took.
        setQuery("");
        setPkg("");
        void search();
        void open({ topic: asked.topic, package: asked.package });
      },
    ),
  );
  // Once the session is idle again, what waited is read.
  const unwatch = ws.runs.watchAll((run) => {
    if (run.session !== id() || run.quiet || run.phase !== "done" || !waiting()) return;
    setWaiting(false);
    void search();
    const place = pending;
    pending = undefined;
    if (place) void open(place);
  });
  onCleanup(() => {
    unwatch();
    clearTimeout(timer);
  });

  const scope = () => (pkg().trim() ? `in ${pkg().trim()}` : python() ? "in imported modules" : "in all installed packages");
  const shownTopics = () => {
    const topics = found()?.topics ?? [];
    return all() ? topics : topics.slice(0, FEW);
  };
  const hidden = () => (found()?.topics.length ?? 0) - shownTopics().length;
  const isOpen = (topic: string, p: string) => page()?.topic === topic && page()?.package === p;

  let body: HTMLDivElement | undefined;

  /** A link in an R page: followed here, run in the Console, or opened in the browser. */
  const onPageClick: JSX.EventHandler<HTMLDivElement, MouseEvent> = (e) => {
    const a = (e.target as HTMLElement).closest("a");
    if (!a) return;
    // Nothing in a page may move the app's own window.
    e.preventDefault();
    const link = helpLink(a.getAttribute("href") ?? "", page()?.package);
    if (!link) return;
    if (link.kind === "web") props.openLink(link.url);
    else if (link.kind === "anchor") body?.querySelector(`[id="${CSS.escape(link.id)}"]`)?.scrollIntoView({ block: "start" });
    else if (link.kind === "example") {
      const session = id();
      if (session) props.onRun(session, exampleCode(link.topic, link.package));
    } else if (link.kind === "index") {
      setQuery("");
      setPkg(link.package ?? "");
      void search();
    } else void open({ topic: link.topic, package: link.package });
  };

  return (
    <div class="helppane">
      <div class="hsearch">
        <Show when={props.sessions.length > 1}>
          <SessionPicker ws={ws} session={props.session} sessions={props.sessions} usedBy={props.usedBy} onPick={props.onSession} />
        </Show>
        <label class="hs-q">
          <Icon name="search" small />
          <input
            placeholder="Function or topic"
            aria-label="Function or topic"
            value={query()}
            onInput={(e) => {
              setQuery(e.currentTarget.value);
              searchSoon();
            }}
            onKeyDown={(e) => {
              if (e.key === "Enter") {
                const typed = query().trim();
                const first = found()?.topics[0];
                // `?lm` or `np.mean?` work here too.
                const asked = consoleHelp(typed, language());
                if (asked && !asked.search && asked.topic) void open(asked);
                else if (first) void open({ topic: first.topic, package: first.package });
                else if (typed) void open({ topic: typed, package: pkg().trim() || undefined });
              } else if (e.key === "Escape") {
                setQuery("");
                searchSoon();
              }
            }}
          />
        </label>
        <label class="hs-p" title={python() ? "A module, like scanpy" : "A package, like Seurat"}>
          <Icon name="pkg" small />
          <input
            placeholder={python() ? "All modules" : "All packages"}
            aria-label="Package"
            value={pkg()}
            onInput={(e) => {
              setPkg(e.currentTarget.value);
              searchSoon();
            }}
          />
        </label>
      </div>
      <div class="hbody" ref={body}>
        <Show
          when={props.session}
          fallback={<p class="vempty">Start R or Python in the Console, and its help shows here.</p>}
        >
          <Show when={waiting()}>
            <div class="vbusy">{name()} is running code; help shows when it finishes.</div>
          </Show>
          <Show when={searchError()}>
            <div class="var-note err">{searchError()}</div>
          </Show>
          <Show
            when={query().trim() || pkg().trim()}
            fallback={
              <Show when={!page() && !pageError()}>
                <div class="hr-n">
                  Type a function name. Leave the {python() ? "module" : "package"} empty to search every {python() ? "imported module" : "installed package"}.
                </div>
              </Show>
            }
          >
            <Show when={found()}>
              {(result) => (
                <div class="hres">
                  <div class="hr-n">
                    {(result().topics.length + result().more).toLocaleString()} topic{result().topics.length + result().more === 1 ? "" : "s"} {scope()}
                  </div>
                  <For each={shownTopics()}>
                    {(t) => (
                      <button class="hr" classList={{ on: isOpen(t.topic, t.package) }} title={t.title} onClick={() => void open({ topic: t.topic, package: t.package })}>
                        <span class="hl-l" classList={{ py: python() }}>
                          {python() ? "Py" : "R"}
                        </span>
                        <span class="hr-f">{t.topic}</span>
                        <span class="hr-p">{t.package}</span>
                      </button>
                    )}
                  </For>
                  <Show when={hidden() > 0}>
                    <button class="hr-n hr-more" onClick={() => setAll(true)}>
                      {(hidden() + result().more).toLocaleString()} more
                    </button>
                  </Show>
                  <Show when={all() && result().more > 0}>
                    <div class="hr-n">… and {result().more.toLocaleString()} more; type more of the name to find them.</div>
                  </Show>
                  <Show when={!result().topics.length}>
                    <div class="hr-n">
                      No help topics match "{query().trim()}" {scope()}.
                    </div>
                  </Show>
                </div>
              )}
            </Show>
          </Show>
          <Show when={pageError()}>
            <div class="helpdoc">
              <p class="var-note err">{pageError()}</p>
            </div>
          </Show>
          <Show when={page()}>
            {(p) => (
              <div class="helpdoc" classList={{ loading: loading() }}>
                <div class="hd-top">
                  <div class="hd-name">
                    <h3>{p().topic}</h3>
                    <div class="pkg">
                      {[p().package, p().version, p().kind].filter(Boolean).join(" · ")}
                    </div>
                  </div>
                  <button class="tab-btn" title="Back" disabled={at() <= 0} onClick={() => void open(trail()[at() - 1], at() - 1)}>
                    <Icon name="chevronLeft" small />
                  </button>
                  <button class="tab-btn" title="Forward" disabled={at() >= trail().length - 1} onClick={() => void open(trail()[at() + 1], at() + 1)}>
                    <Icon name="chevronRight" small />
                  </button>
                  <button class="vbtn hd-src" aria-pressed={showSource()} title={showSource() ? "Show the help" : "Show the function's source"} onClick={() => void toggleSource()}>
                    <Icon name="source" small />
                    {showSource() ? "Help" : "Source"}
                  </button>
                </div>
                <Show when={p().alternatives?.length}>
                  <p class="hd-also">
                    Also in{" "}
                    <For each={p().alternatives}>
                      {(alt, i) => (
                        <>
                          {i() ? ", " : ""}
                          <a href="#" title={alt.title} onClick={(e) => (e.preventDefault(), void open({ topic: alt.topic, package: alt.package }))}>
                            {alt.package}
                          </a>
                        </>
                      )}
                    </For>
                  </p>
                </Show>
                <Show when={!showSource()} fallback={<SourceView source={source()} language={language()} />}>
                  <Show when={p().html} fallback={<PythonDoc page={p()} onTopic={(ref) => void follow(ref)} onMember={(m) => void open({ topic: `${p().topic}.${m}` })} openLink={props.openLink} />}>
                    {(html) => <div class="rdoc" innerHTML={sanitize(html())} onClick={onPageClick} />}
                  </Show>
                </Show>
              </div>
            )}
          </Show>
        </Show>
      </div>
    </div>
  );
}

/** An R page's HTML, without scripts, styles or images that can't load. */
function sanitize(html: string): string {
  const clean = DOMPurify.sanitize(html, { USE_PROFILES: { html: true }, FORBID_TAGS: ["style", "link", "img"] });
  return clean;
}

function SourceView(props: { source?: HelpSource | string; language: string }) {
  return (
    <Show when={props.source} fallback={<p class="hr-n">Reading the source…</p>}>
      {(s) => {
        const value = s();
        if (typeof value === "string") return <p class="var-note err">{value}</p>;
        return (
          <>
            <Show when={value.file}>
              <p class="hr-n hd-file" title={value.file}>
                <span>
                  {value.file}
                  {value.line ? `:${value.line}` : ""}
                </span>
              </p>
            </Show>
            <pre class="hsrc">{highlightedLines(value.text.replace(/\s+$/, ""), languageFor(props.language))}</pre>
          </>
        );
      }}
    </Show>
  );
}

const MEMBER_GROUPS: [string, string[]][] = [
  ["Classes", ["class"]],
  ["Functions", ["function"]],
  ["Methods", ["method"]],
  ["Properties", ["property"]],
  ["Modules", ["module"]],
  ["Other", ["value"]],
];

/** A Python page: its signature, its docstring as sections, and its public members. */
function PythonDoc(props: { page: HelpPage; onTopic: (ref: string) => void; onMember: (name: string) => void; openLink: (url: string) => void }) {
  const inline = (text: string) => (
    <For each={inlineParts(text)}>
      {(part) =>
        "code" in part ? (
          <code>{part.code}</code>
        ) : "em" in part ? (
          <em>{part.em}</em>
        ) : "ref" in part ? (
          <a href="#" onClick={(e) => (e.preventDefault(), props.onTopic(part.ref))}>
            <code>{part.label}</code>
          </a>
        ) : "url" in part ? (
          <a href={part.url} title={part.url} onClick={(e) => (e.preventDefault(), props.openLink(part.url))}>
            {part.label}
          </a>
        ) : (
          part.text
        )
      }
    </For>
  );
  /** A See Also entry's names ("merge_ordered, merge_asof : …") as links. */
  const seeAlso = (term: string) => {
    const [names, rest] = term.split(/\s+:\s+/, 2);
    return (
      <>
        <For each={names.split(/,\s*/)}>
          {(n, i) => (
            <>
              {i() ? ", " : ""}
              <a href="#" onClick={(e) => (e.preventDefault(), props.onTopic(n.trim()))}>
                {n.trim()}
              </a>
            </>
          )}
        </For>
        {rest ? <span class="hm-t"> : {rest}</span> : ""}
      </>
    );
  };
  const groups = () =>
    MEMBER_GROUPS.map(([title, kinds]) => [title, (props.page.members ?? []).filter((m) => kinds.includes(m.kind))] as [string, HelpMember[]]).filter(
      ([, members]) => members.length,
    );
  return (
    <div class="pydoc">
      <Show when={props.page.signature}>{(sig) => <pre class="hsig">{signatureLines(props.page.topic, sig())}</pre>}</Show>
      <Show when={props.page.text} fallback={<p class="hr-n">No documentation.</p>}>
        <For each={docBlocks(props.page.text ?? "")}>
          {(block) => {
            switch (block.kind) {
              case "heading":
                return <h4>{block.text}</h4>;
              case "code":
                return <pre class="hcode">{highlightedLines(block.text, languageFor("python"))}</pre>;
              case "note":
                return (
                  <div class="hnote">
                    <b>{block.title}</b>
                    <Show when={block.text}>
                      <p class="hpara">{inline(block.text)}</p>
                    </Show>
                  </div>
                );
              case "defs":
                return (
                  <dl>
                    <For each={block.items}>
                      {(item) => (
                        <>
                          <dt>{/^see also$/i.test(block.section) ? seeAlso(item.term) : item.term}</dt>
                          <Show when={item.text}>
                            <dd>{inline(item.text)}</dd>
                          </Show>
                        </>
                      )}
                    </For>
                  </dl>
                );
              default:
                return <p class="hpara">{inline(block.text)}</p>;
            }
          }}
        </For>
      </Show>
      <For each={groups()}>
        {([title, members]) => (
          <>
            <h4>{title}</h4>
            <div class="hmembers">
              <For each={members}>
                {(m) => (
                  <button class="hmem" title={m.title} onClick={() => props.onMember(m.name)}>
                    <span class="hr-f">{m.name}</span>
                    <span class="hm-t">{m.title}</span>
                  </button>
                )}
              </For>
            </div>
          </>
        )}
      </For>
    </div>
  );
}
