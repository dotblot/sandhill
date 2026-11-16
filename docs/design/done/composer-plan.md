# Redesigning the message box

Huan, 2026-11-16: "redesign the composer box. I'm not sure everything is
functional. Maybe add or remove something to accommodate sandhill."
Status: approved and built 2026-11-16, with Huan's answers below.

## Huan's answers (2026-11-16)

1. Web search: no switch. Each agent keeps its own default.
2. Follow edits: removed from the box (stays in Welcome → General).
3. Code goes File / Console: its own pill in the box.
4. Modes: each agent's own names, starting in its own default. Sandhill's
   own Restricted and Ask are dropped. pi has no modes: no pill, it runs
   everything as it does by itself. Antigravity gets all five of its own.
5. A + button.

Each agent's menu (bold: where it starts), and what Sandhill sends the
daemon (`AgentOptions::permission`):

| Agent | Modes |
|---|---|
| Claude Code | **Manual** `supervised`, Accept edits `edits`, Plan mode `plan`, Auto mode `auto`, Bypass permissions `full` |
| Codex | Read Only `readonly`, **Default** `supervised`, Approve for me `auto`, Full Access `full`, Plan mode `plan` |
| Antigravity | **Default** `supervised`, Accept edits `edits`, Auto `auto`, Plan `plan`, Skip permissions `full` |
| OpenScience | **Build** `supervised`, Plan `plan` |
| Oh My Pi | **Always ask** `supervised`, Write `edits`, YOLO `full`, Plan `plan` |
| pi | none |
| Posit Assistant | its own screen, unchanged |

`supervised` is always the agent's own default. Changes from before:
Codex's default edits the project without asking (it started read-only);
OpenScience runs Sandhill's tools without asking; pi never asks. The
notebook's "Explain" shortcut, which used Ask mode, starts in Read Only
(Codex) or Plan (the others).

The message box is the box at the bottom of the AI pane
(`agents/composer_view.rs::render_composer`).

## What is there now, and whether it works

Checked by reading the code, not by running each agent.

Works as meant:
- Typing, Enter to send, Shift+Enter for a new line, growing to 8 lines.
- `@` (files, cells, variables, plots, "Attach from this computer…") and
  `#` (cells of a notebook already added). What is picked becomes a chip
  with ×; Backspace at the start removes the last chip.
- Pasting an image or files, and dragging files in from Finder.
- The round button: send, or stop while the agent works and nothing is
  typed.
- The context ring (how full the conversation is) with "Compact".
- The model and effort picker.

Weak or misleading:
1. **Web search (globe).** Only Claude Code is really stopped from
   searching (`terminal/events.rs` turns its WebSearch and WebFetch down).
   Codex, Antigravity, OpenScience, Oh My Pi and pi are only sent a line
   asking them not to (`choices::WEB_OFF_NOTE`); nothing stops them. The
   button has no tooltip.
2. **Follow edits (magnifying glass).** It is a setting of this Mac for
   every project, and is already in Welcome → General ("Follow the AI's
   edits"). Here it looks like a choice for this conversation, and its
   icon is the one Plots uses for zoom.
3. **Mode.** Every agent is offered the same six modes, but:
   - pi: Auto runs everything, the same as YOLO (`pi/mod.rs::gate`); the
     menu says "A second model reviews risky steps", which pi doesn't do.
   - Oh My Pi's own command line: Auto, Restricted, Ask and Plan all map
     to "always ask" (`acp/mod.rs::omp_approval`).
   - In Normal the button is an icon with no name.
4. **Where the agent's code goes (file or Console).** A Sandhill choice
   that matters a lot, hidden at the bottom of the mode menu.
5. **Adding things.** Nothing in the box says `@` and `#` exist; the hints
   are only on the new-conversation screen.

## The new box

One rounded box, in the status bar's language (20px capsules, 13px text):

- **Chips** on top, as now.
- **Text** across the full width (the send button moves down).
- **Bottom row, left:**
  - **+** opens the `@` list (files, cells, variables, plots, attach from
    this computer). Typing `@` and `#` still works.
  - **Mode** capsule, always with its name ("Normal", "Plan", …). Its menu
    lists only the modes that mean something different for that agent,
    with wording true for that agent.
  - **Code goes** capsule: "File" or "Console", clicking switches. Hidden
    for agents that can't write into Sandhill's documents.
  - **Web** globe, with a tooltip, shown only for agents where off is
    really enforced (today Claude Code).
- **Bottom row, right:** context ring, model capsule, send/stop.

Removed: the follow-edits button (it stays in Welcome → General).

## Not changed

What `@`, `#`, paste, drop, send, stop, the model picker and the context
popup do; "no @ symbol shown in the box" and "no implicit document context"
(`docs/decisions/agents.md`).

## Questions for Huan

1. Web search: show the globe only where off is enforced (Claude Code
   now), or keep it for all and try to enforce it per agent (more work,
   and some agents may have no such switch)?
2. Remove the follow-edits button from the box (kept in General)?
3. "Code goes: File / Console" as its own capsule in the box?
4. Mode menu: list only the modes that truly differ for each agent?
5. Add the + button?
