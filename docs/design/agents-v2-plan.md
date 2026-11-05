# Agents pane, version 2: plan

Status: approved (Huan answered "1A, 2 file, 3 yes, 4 Posit, 5 yes"). Steps 1 and 3
are built (PRs #24 and #25); the chat pane waits for the draft 18 mockup.
Revision 3, 2026-10-12.
- Revision 1 answered the five issues Huan found testing the draft 17 build
  (PR #23).
- Revision 2 follows Huan's direction that the AI chat should be "very similar
  to Posit Assistant, including the looking and feels as well as how it works".
- Revision 3 refines the look from Huan's screenshot of Posit Assistant in
  Positron.
- Build only after Huan approves.

What already works, per Huan's test: sessions start, the agent made and showed a
data frame, and both the session and the data frame were still there after
quitting and reopening Sandhill.

## The short version

- **One chat pane, laid out and working like Posit Assistant.** It is the same
  for Claude Code and Codex, and both stay signed in with Huan's own plans. The
  terminal goes away, apart from a toggle for the rare screen the chat can't
  show yet.
- **Most of Posit Assistant carries over.**
  - Fully: the chat with code blocks and plots, running code in your live R and
    Python sessions, your variables and open files sent with every message, the
    mode picker, approvals with "once / this conversation / this project",
    conversation history, export and `/report`.
  - Partly, for Claude: replies arrive a line at a time rather than word by
    word, and plan approval and multiple-choice questions need a quick test
    before they can be cards.
  - Not for Claude: branching from an earlier message.
- **Huan's five issues are all covered.**
  1. The linked file comes to the front.
  2. There is one message box.
  3. A mode picker that really switches modes.
  4. Agent code goes into the file below the active cell, or into the
     Console.
  5. The chat pane itself.
- **The look** comes from Huan's screenshot of Posit Assistant in Positron
  ([posit-assistant-positron.png](posit-assistant-positron.png), added in
  revision 3) and Posit's docs. Posit's own screenshots couldn't be downloaded
  from here.

---

## 1. What Posit Assistant is and how it works

Sources: [Posit Assistant docs](https://assistant.posit.co/docs/getting-started/)
(Features and Guides sections), Posit's
[comparison with Claude Code](https://opensource.posit.co/blog/2026-06-08_comparing-posit-assistant-and-claude-code/),
and its [feedback forum](https://github.com/posit-dev/assistant-feedback/discussions/11).

**What it is.**
- Posit's own agent. It isn't Claude Code; it talks to the models itself.
- It runs inside Positron (an activity-bar sidebar, or an editor tab) and
  RStudio, and as a terminal app (`pa`).
- Models come through Posit AI Pass (from $20 a month of credit) or API keys:
  Anthropic, OpenAI, Gemini, Ollama and others. Its provider list doesn't
  include Claude or ChatGPT subscription plans.
- It replaced Positron Assistant and Databot in mid-2026.

**The chat.**
- Messages in Markdown. Each step the agent takes is a tool block that shows
  its code with syntax highlighting, so each step can be checked.
- Plots appear in the chat with the code that made them, and in the Plots pane.
  The model sees the plot too.
- It explores in small steps: a few snippets of code, a summary, then suggested
  next steps.
- In a data cleaning mode it asks questions with options to pick from, in tabs.
- The input takes files: drag, paste, the paperclip, or `@` to name a file.
- Under the input, in one row: a globe for web search, a session-context
  toggle, the approval mode, the linked file with its kernel, tokens and cost,
  and the model with a "Thinking effort" submenu (Off, Low, Medium, High). The
  token counter opens a per-turn usage view.
- Selecting text in the chat shows Copy, Quote in chat and Add to memory.
- Tips appear above the input.

**Using your session.**
- Every message carries the session's language, version, and variable names and
  types (not values).
- It reads loaded packages, data frames, console history and open files (open
  files are included by default).
- Its `runCode` tool runs code in your live session. The code and its output
  appear in the Console, which users find easy to follow; they have asked for
  clearer output and a way to re-run the agent's code.

**Files.**
- It reads and edits files. Edits show in the chat as diffs and are accepted all
  together; users have asked to review each change separately.
- `/report` turns the conversation into a Quarto document, and `/notebook` into
  a Jupyter notebook.

**Modes and approvals.**
- Approval modes:
  - Normal: safe steps run, risky ones ask.
  - Auto: a second model reviews risky steps.
  - YOLO: everything runs except explicitly dangerous steps.
  - Restricted: asks before everything.
- An approval can be given once, for the conversation, or for the project.
  Each tool can be set to allow, ask or deny.
- First-time projects ask to be trusted. A sandbox can confine shell commands.
- **Ask mode** is read-only: no edits and no code runs. The agent suggests
  changes in the chat instead.
- **Plan mode** writes a plan file, then asks "Exit and implement" or "Keep
  planning".

**Conversations.**
- A history panel to switch between conversations, rename and delete them.
- Editing an earlier message starts a branch.
- Export to Markdown, HTML or JSON, and import again.
- Compaction, both automatic and `/compact`.
- A notification when a background conversation finishes or needs you.

**Memory, skills, tools.**
- Memory: an `AGENTS.md` in the project and one for the user; `/savememory`
  writes them.
- Skills in the Agent Skills format (`SKILL.md`), with 11 built in, such as
  `report` and `quarto-authoring`.
- MCP servers, and hooks in Claude Code's style.

---

## 2. Posit Assistant, mapped to Sandhill

Underneath, Sandhill keeps Claude Code and Codex: Codex through
`codex app-server`, and Claude through a chat view on the real Claude Code
session (section 3 explains why). ✓ works, ◐ partly or needs a check, ✗ not
possible.

| Posit Assistant | How Sandhill does it | Codex | Claude |
|---|---|---|---|
| Chat with Markdown and code blocks | One chat pane in the Agents slot | ✓ | ✓ |
| Streaming replies | | ✓ word by word | ◐ a line at a time |
| Tool rows showing the code | One line per step, expanding to code, output and plots (see The look) | ✓ | ✓ |
| "Thought briefly" rows | Collapsed thinking rows between steps | ✓ reasoning summaries | ◐ none: hooks don't carry Claude's thinking, so a "Thinking…" row shows only while it works |
| Plots in the chat, seen by the model | Sandhill's run tools already capture plots; the card shows them. The Plots pane comes with step 5 | ✓ | ✓ |
| Re-run, copy (users' request) | Run again, Copy and Insert into file on every code card | ✓ | ✓ |
| Runs code in your live session, visible in the Console | Sandhill's `run_code` tool, now echoed in the Console and marked as the agent's | ✓ | ✓ |
| Session context with every message | Language, version, variable names and types, recent console lines, open files, active file and cell; the eye toggle turns it off | ✓ added to each turn | ✓ through a documented hook (`UserPromptSubmit` can add context) |
| Code into your file | `add_cell`: below the active cell in the linked file, run there (Huan's point 4) | ✓ | ✓ |
| File edits as diffs | Each edit is its own card with Allow / Deny, so changes are reviewed one at a time | ✓ | ✓ |
| Approval modes Normal, Auto, YOLO, Restricted | One mode picker (the hand icon under the input); see point 3 | ✓ | ✓ (Restricted through a hook) |
| Approve once / conversation / project | Three buttons on the request | ◐ once, conversation | ✓ |
| Ask mode (read-only) | Sandhill blocks edits and code runs | ✓ read-only sandbox | ✓ through a hook |
| Plan mode, "Exit and implement" | | ◐ shows the plan's steps, no approval step | ◐ Claude's own plan mode; the approval should come through as a request (needs a test) |
| Questions with options (data cleaning) | Option cards | ◐ | ◐ Claude's question tool; needs a test, else the terminal opens |
| Model picker with thinking effort | The draft 17 effort knob (Huan's design) instead of a submenu | ✓ | ✓ |
| Web search toggle | | ◐ set per conversation | ◐ Sandhill blocks the search tools when off |
| Tokens and cost per turn and per conversation | Tokens per turn, and context use for the conversation. Plans aren't billed per message, so plan usage rings take the place of the dollar amounts | ✓ | ✓ |
| Attach images and files | Saved on the server and passed in | ✓ | ◐ as a file Claude reads |
| Selection toolbar (copy, quote, add to memory) | | ✓ | ✓ |
| Conversation history: switch, rename, delete | Sandhill's own list; reopening resumes the agent's conversation | ✓ | ✓ |
| Branch by editing an earlier message | | ◐ Codex can fork a conversation | ✗ (Claude Code only offers this in its terminal menu) |
| Export Markdown, HTML, JSON; `/report`, `/notebook` | Export from Sandhill's saved conversation; `/report` and `/notebook` ask the agent to write the document with `add_cell` | ✓ | ✓ |
| Compaction | The context bar's compact button | ◐ automatic | ✓ (`/compact`) |
| Memory: `AGENTS.md`, `/savememory` | `/savememory` asks the agent to update it | ✓ reads `AGENTS.md` | ✓ reads `CLAUDE.md`; Sandhill points it at `AGENTS.md` too |
| Skills, MCP servers, hooks | The agents' own; later, Sandhill can ship data-analysis skills passed on the command line | ✓ | ✓ |
| Trust a new project, sandbox | The agents' own trust question, shown as a card | ✓ sandbox built in | ◐ |
| Notification when done or waiting | macOS notification, plus the "waiting for you" notice | ✓ | ✓ |
| Tips above the input | Left out | | |

**The look**, following Huan's screenshot
([posit-assistant-positron.png](posit-assistant-positron.png)). Sandhill keeps
draft 17's style: fonts, colours, glass panes, the effort knob and the usage
rings. Inside it, the chat follows Posit's layout.

Placement: Posit Assistant sits in Positron's left sidebar, between the
activity bar and the editor. That is where Sandhill's Agents pane already is.

Top to bottom:
- **Header:** the pane title, then a row with + (new conversation), a clock
  (conversation history) and a gear (settings). This replaces today's tab per
  session. Several conversations still run at once: the history list marks the
  busy ones and those waiting for you.
- **Your messages:** grey rounded bubbles. Long pasted text, such as an error,
  stays in the bubble.
- **Agent replies:** plain text, without a bubble. Inline code is a small grey
  pill in the code font.
- **Thinking rows:** "Thought briefly ›" in small grey text, collapsed, opening
  to the reasoning.
- **Step rows:** one line each, collapsed:
  - a status icon (a check when done, an x when cancelled or failed, a spinner
    while running);
  - the step's name in bold code font, then a short plain description written
    by the agent, for example "executeCode: Check whether the requested genes
    exist in any assay of srt";
  - a detail on the right: "lines 90–119 of 351" for a read, "+1 −0" for an
    edit;
  - a › chevron that opens the code, its output and plots, or the diff.
  - File names are links that open the file at those lines.
  - For Sandhill's own tools the row reads, for example, "Ran in analysis.qmd
    (Python)", or "Added a cell below 'Load and filter'". The opened row has
    Run again, Copy and Insert into file.
- **Approval:** a request appears where the step will run, with Allow and Deny
  and the once / conversation / project choice. Denying shows the row with an
  x, and the agent asks what to do instead, as in the screenshot.
- **Turn footer:** a copy button, the time, and the turn's tokens. Posit's cost
  in dollars is left out, because plan sign-ins aren't billed per message.
- **Input box:** rounded, with a paperclip on the left, "Ask Claude…" or "Ask
  Codex…" as the placeholder, and a round send button (a stop button while the
  agent works). Its bottom row holds:
  - on the left: web search (globe), session context on or off, and the
    approval mode (hand);
  - on the right: the linked file chip with its kernel ("1_QC.ipynb (Python)",
    which opens the file; point 1), the context use, and the model with its
    effort ("Opus (High) ⌄", which opens draft 17's model menu with the effort
    knob).
- **Plan usage rings** stay in the status bar, as in draft 17.

The screenshot also shows Posit's right sidebar with Variables and Plots: that
is Sandhill's right pane, which comes with step 5.

Posit Assistant isn't open source, so nothing of its code or images is
reused. Huan's process is design first, so a mockup (draft 18) from the
design thread should come before the build (Q3).

---

## 3. What runs underneath, and Anthropic's and OpenAI's terms

**Codex: `codex app-server`.** OpenAI documents it for embedding Codex in other
apps ([docs](https://developers.openai.com/codex/app-server)). It works with
your ChatGPT sign-in.
- Replies stream word by word, and so do command output, the plan's steps and
  each turn's combined diff.
- Command and file-change approvals arrive as requests Sandhill answers.
- Approval policy, sandbox, model and effort are set with each message. The
  model list says which effort levels each model has.
- It reports plan limits and token counts.
- Codex's own terminal interface can attach to the same conversation
  (`codex --remote`), which is the Terminal toggle.
- Watch out for three things:
  - It is marked "experimental", so Sandhill pins and checks the Codex version.
  - OpenAI allows app-server sign-in for local and open-source apps, which
    Sandhill is, but "never … for commercial or hosted services".
  - Only one app-server may write to a conversation at a time.

**Claude: a chat view on the real Claude Code session.**
- Anthropic's terms
  ([legal page](https://code.claude.com/docs/en/legal-and-compliance)) say apps
  "using the Agent SDK should use API key authentication". Claude Code's
  headless mode (`claude -p`) is "using the Agent SDK via the CLI". Signing in
  "to the unmodified Claude Code binary with their own Claude subscription" is
  the case they allow.
- So Claude Code keeps running exactly as now: the unmodified program, signed in
  by you, in a terminal the daemon keeps alive. The terminal is hidden behind
  the chat.
- The chat is built from Claude Code's documented hooks, which Sandhill already
  uses for the busy dot and the status line:
  - `UserPromptSubmit`: your messages, and adds the session context to each one.
  - `MessageDisplay`: Claude's replies, a few lines at a time.
  - `PreToolUse` / `PostToolUse`: each step, with its input and result. These
    also enforce Ask and Restricted modes.
  - `PermissionRequest`: the Allow / Deny cards, with "always" rules for the
    conversation or project.
  - `Stop`: the end of each turn.
  - The status line: model, effort, context and plan usage.
- Your messages and `/model`, `/effort` and mode changes are typed into the
  session for you.
- The daemon saves every event, so the conversation replays after reconnecting.
- Limits:
  - Replies arrive a line at a time.
  - Claude's thinking isn't shown.
  - No branching.
  - Plan approval, multiple-choice questions and menus like `/config` may open
    the terminal until each is tested as a card.
  - A future Claude Code version could change a hook; the terminal toggle always
    works as a fallback.

**Other ways for Claude (for Q1).**
- B: a chat pane on Claude Code's headless mode with an Anthropic API key. This
  is the smoothest route (word by word, documented controls), but billed per use,
  separate from the Pro plan.
- C: keep the terminal for Claude and build the chat pane only for Codex.
- **Later option:** Sandhill's own agent, built like Posit Assistant, that talks
  to the model APIs directly with API keys (already in `docs/design.md` §5 as
  "optional later"). It would match Posit Assistant exactly, branching included,
  but it is pay per use and the largest build.

---

## 4. Huan's five issues under this design

**1. The linked file comes to the front.** Today "Refers to" only reaches the
agent's instructions (`crates/daemon/src/agents.rs`) and the file names under
the tab; switching sessions never tells the editor (`onSelect={setAgentTab}` in
`App.tsx`).
- Starting a session, or switching to it, opens and activates its linked file.
- The file names in the chat header become buttons.
- "@ Add context" also links the file to the session.
- Editor tabs of linked files get a small agent marker.
- Switching editor tabs doesn't switch the chat, so the pane doesn't jump while
  you work.

**2. One message box.** The chat pane has only Sandhill's input, with `@`
files, attachments, model and effort, and web search. The agent's own input
exists only behind the Terminal toggle.

**3. A mode picker that really switches.**
- Today the button shows only the starting mode. "Supervised" also passes no
  setting, so Claude uses your own Claude default; that's why Claude said
  "auto mode on" while the button said Supervised.
- The new picker, the hand icon under the input as in Posit's, offers Normal,
  Auto, YOLO, Restricted, Ask and Plan (names in Q4).
- It shows the mode the agent reports, and it switches modes for real:
  - **Codex:** set with the next message.
  - **Claude:** Sandhill presses ⇧Tab for you until Claude reports the chosen
    mode. Restricted and Ask are enforced by Sandhill's own hook.
  - Every session starts with an explicit mode.
  - YOLO for Claude needs the session started with it allowed; switching to it
    restarts Claude on the same conversation, with a warning.

**4. Agent code into the file.**
- Sandhill tells the daemon where you are: the active file, the active cell
  (the chunk with the cursor in `.qmd`/`.Rmd`, the selected cell in `.ipynb`)
  and the selection. The daemon remembers it, even with the Mac closed.
- A new tool, `add_cell`, writes the agent's code as a new cell below your
  active cell in the linked file, or after the last cell if there is no active
  cell, and runs it with the file's own session.
  - If the file is open, the Sandhill window makes the edit (one ⌘Z undoes it),
    brings the tab forward, and runs it as if you pressed Run. The output
    appears under the cell.
  - If the file isn't open (the Mac is closed), the daemon edits the file and
    runs the cell itself.
- The output, errors and plots go back to the agent and show on the chat card.
  A failed cell is fixed in place (`edit_cell`), not repeated as new cells.
- A per-conversation switch picks where code goes: "In the file" or "In the
  Console". The default is Q2.
- Quick looks (`head()`, `dim()`) use `run_code`, shown in the Console and
  marked as the agent's.
- `get_focus` lets the agent see your place, so "fix this cell" works.
- In Normal mode, the agent asks before each `add_cell`, and the request shows
  the code.

**5. The chat pane:** sections 1 to 3.

---

## 5. How it would be built

- **Daemon:**
  - One conversation model for both agents: messages, step cards (code, output,
    plots, diffs), approval requests, questions, plan, usage and end of turn.
    Codex's app-server events and Claude's hook events are translated into it.
  - It is saved to disk like kernel outputs, so the app replays it after
    reconnecting, and the agent keeps working with no app connected. A request
    waits for you and raises the "waiting for you" notice.
  - New calls: send, answer a request, stop, set model / effort / mode / web
    search, list and resume conversations, and export.
- **Session tools:** `add_cell`, `edit_cell`, `get_focus`, console echo of
  agent runs, and the per-turn session context. Both agents use them, so they
  come first.
- **App:**
  - The chat list, drawing only what is on screen.
  - Markdown and code rendering, step cards with plots, and the diff view
    (CodeMirror's merge view).
  - Approval and question cards, the input row, the history panel, the mode
    picker and the Terminal toggle.
- **Tests:**
  - Recorded event streams for both translations.
  - Real-kernel tests for the session tools (CLAUDE.md).
  - Real-agent tests that skip when a CLI isn't installed.
- **Size:** the largest piece of work so far. It is done once for the shared
  parts, then Codex, then Claude.

## Order of work

1. **Small build now:** the linked file comes to the front, and every session
   starts with an explicit mode, fixing the wrong "Supervised" label. **Done (PR #24).**
2. **Mockup:** the design thread draws the Posit-style chat pane in Sandhill's
   style (draft 18).
3. **Session tools:** `add_cell`, `edit_cell`, `get_focus`, Console echo and
   session context. **Done in PR #25, except Console echo,** which waits for
   a session-wide Console (today each script has its own).
4. **Chat pane with Codex** on app-server.
5. **Claude in the chat pane**, as decided in Q1.

## Questions for Huan

**Q1. What runs Claude in the chat pane?**
- A. The real Claude Code with your plan (a line at a time; a few screens open
  the terminal at first). **Recommended.**
- B. Claude Code's headless mode with an Anthropic API key: smoother, billed per
  use.
- C. Keep the terminal for Claude; chat pane for Codex only.

**Q2. Where does the agent's code go by default?**
- In the file, below the active cell, as you asked. **Recommended.**
- In the Console, as Posit Assistant does; `/report` turns the chat into a
  document afterwards.
Quick looks like `head()` go to the Console either way.

**Q3. Mockup first?**
- Yes: the design thread draws the chat pane (draft 18) before it is built.
  **Recommended.**
- No: build straight from Posit's layout.

**Q4. Mode names?**
- Posit's: Normal, Auto, YOLO, Restricted, Ask, Plan. **Recommended**, since you
  want it to feel like Posit Assistant.
- Sandhill's current ones: Supervised, Auto-accept edits, Auto, Full access, plus
  Ask and Plan.

**Q5. Ship the two small fixes (linked file, explicit mode) now?**
- Yes, as a small build. **Recommended.**
- No, wait for the chat pane.

Chosen defaults, no question needed: a failed cell is fixed in place;
switching editor tabs doesn't switch the chat; there are no tips above the
input.
