import { createMemo, createSignal, For, Show } from "solid-js";
import type { GitOwnCommits, GitRewrite } from "../backend/types";
import { Icon } from "./icons";

/**
 * Squash and reorder: the branch's own commits, newest at the top as in the
 * graph, to drag into another order or fold into the commit below. A commit
 * that others fold into gets their messages under it, to edit. Answers the
 * commits in their new order, oldest first, or undefined to cancel.
 */
export function RewriteDialog(props: { own: GitOwnCommits; onDone: (commits: GitRewrite[] | undefined) => void }) {
  const messages = new Map(props.own.commits.map((c) => [c.sha, c.message]));
  const start = [...props.own.commits].reverse().map((c) => c.sha);
  /** Newest first, as shown. */
  const [order, setOrder] = createSignal(start);
  const [squashed, setSquashed] = createSignal<ReadonlySet<string>>(new Set());
  /** Messages typed, by the commit the others fold into. */
  const [typed, setTyped] = createSignal<ReadonlyMap<string, string>>(new Map());
  const [dragging, setDragging] = createSignal<string>();

  /** The bottom commit has nothing below it to fold into. */
  const squashes = (sha: string, at: number) => squashed().has(sha) && at < order().length - 1;
  /** Oldest first: each commit with the ones folded into it. */
  const groups = createMemo(() => {
    const list: { sha: string; folded: string[] }[] = [];
    const oldest = [...order()].reverse();
    oldest.forEach((sha, i) => {
      const at = oldest.length - 1 - i;
      if (squashes(sha, at) && list.length) list[list.length - 1].folded.push(sha);
      else list.push({ sha, folded: [] });
    });
    return list;
  });
  const groupOf = (sha: string) => groups().find((g) => g.sha === sha);
  const joined = (g: { sha: string; folded: string[] }) => [g.sha, ...g.folded].map((s) => messages.get(s)!.trim()).join("\n\n");
  const messageOf = (g: { sha: string; folded: string[] }) => typed().get(g.sha) ?? joined(g);
  const changed = () => order().some((sha, i) => sha !== start[i]) || groups().some((g) => g.folded.length);
  const after = () => groups().length;

  const move = (sha: string, to: number) =>
    setOrder((list) => {
      const rest = list.filter((s) => s !== sha);
      rest.splice(Math.max(0, Math.min(rest.length, to)), 0, sha);
      return rest;
    });
  const toggle = (sha: string) =>
    setSquashed((set) => {
      const next = new Set(set);
      if (!next.delete(sha)) next.add(sha);
      return next;
    });
  const done = () =>
    props.onDone(
      groups().flatMap((g): GitRewrite[] => [
        g.folded.length ? { sha: g.sha, message: messageOf(g) } : { sha: g.sha },
        ...g.folded.map((sha) => ({ sha, squash: true })),
      ]),
    );

  return (
    <div class="modal-backdrop" onKeyDown={(e) => e.key === "Escape" && props.onDone(undefined)}>
      <div class="modal rwdlg" role="dialog" aria-modal="true" aria-label="Squash and reorder commits">
        <p class="modal-message">Squash and reorder commits</p>
        <p class="modal-detail">
          Your commits that aren't pushed yet, newest at the top. Drag one to move it, or squash it into the commit below. Changes not committed are put aside and
          brought back.
          <Show when={props.own.note}> {props.own.note}</Show>
        </p>
        <ol class="rw-list">
          <For each={order()}>
            {(sha, i) => {
              const group = () => groupOf(sha);
              return (
                <li
                  class="rw-row"
                  classList={{ folded: squashes(sha, i()), dragging: dragging() === sha }}
                  draggable={true}
                  onDragStart={(e) => {
                    setDragging(sha);
                    e.dataTransfer?.setData("text/plain", sha);
                  }}
                  onDragEnd={() => setDragging(undefined)}
                  onDragOver={(e) => {
                    const from = dragging();
                    if (!from || from === sha) return;
                    e.preventDefault();
                    move(from, i());
                  }}
                  onDrop={(e) => e.preventDefault()}
                >
                  <div class="rw-line">
                    <span class="rw-grip" aria-hidden="true">
                      ⋮⋮
                    </span>
                    <span class="rw-subject" title={messages.get(sha)}>
                      {messages.get(sha)!.split("\n")[0]}
                    </span>
                    <code class="rw-sha">{sha.slice(0, 7)}</code>
                    <button class="ib" title="Move up" aria-label="Move up" disabled={i() === 0} onClick={() => move(sha, i() - 1)}>
                      <Icon name="up" small />
                    </button>
                    <button class="ib rw-down" title="Move down" aria-label="Move down" disabled={i() === order().length - 1} onClick={() => move(sha, i() + 1)}>
                      <Icon name="up" small />
                    </button>
                    <label class="rw-squash" title="Fold this commit into the one below it">
                      <input type="checkbox" checked={squashes(sha, i())} disabled={i() === order().length - 1} onChange={() => toggle(sha)} />
                      Squash
                    </label>
                  </div>
                  <Show when={group()?.folded.length ? group() : undefined}>
                    {(g) => (
                      <textarea
                        class="rw-message"
                        aria-label="Message of the squashed commit"
                        rows={Math.min(8, messageOf(g()).split("\n").length + 1)}
                        value={messageOf(g())}
                        onInput={(e) => {
                          const text = e.currentTarget.value;
                          setTyped((m) => new Map(m).set(g().sha, text));
                        }}
                      />
                    )}
                  </Show>
                </li>
              );
            }}
          </For>
        </ol>
        <div class="modal-buttons">
          <button class="primary" disabled={!changed() || groups().some((g) => g.folded.length && !messageOf(g).trim())} onClick={done}>
            {after() === start.length ? "Reorder" : `Squash into ${after()} ${after() === 1 ? "commit" : "commits"}`}
          </button>
          <button onClick={() => props.onDone(undefined)}>Cancel</button>
        </div>
      </div>
    </div>
  );
}
