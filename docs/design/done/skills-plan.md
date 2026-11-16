# Skills: install, manage and hand to every agent

Status: **approved by Huan and built, 2026-11-16**. Connectors are a
later plan; Huan chose skills first.

## What Huan asked for
- Installing skills and connectors, similar to Claude Science
  (2026-11-16).
- Skills first.
- All of Claude Science's Featured skills and connectors "if possible"
  (screenshots: 24 skills, 20 Featured connectors).
- Per device, with a "Copy to…" button.

A **skill** is a folder with a `SKILL.md` file (written instructions) and
sometimes scripts and reference files. An agent reads it when the work
calls for it. This is the open "Agent Skills" format that Claude Code,
Codex and others already use.

## What can and cannot come from Claude Science
Checked on 2026-11-16.

- Claude Science's Featured skills and connectors are packed inside its
  program file (`Claude Science.app/Contents/Resources/bin/claude-science`).
  They are not published as files, and no licence lets Sandhill copy them.
  Sandhill is GPL and may only take material under a compatible licence
  (`docs/decisions/licences.md`), so **Sandhill will not take them out of
  the app**.
- What Anthropic has published openly, under Apache-2.0 (compatible):

  | Skill | Where | In Huan's screenshot? |
  |---|---|---|
  | scvi-tools | `anthropics/knowledge-work-plugins`, `bio-research` | yes |
  | single-cell-rna-qc | same | no |
  | nextflow-development | same | no |
  | instrument-data-to-allotrope | same | no |
  | scientific-problem-selection | `anthropics/life-sciences` | no |
  | clinical-trial-protocol | same | no |
  | clinical-trial-forecast | same | no |
  | binder-gauntlet | same | no |

- Not found published anywhere I looked: AlphaFold2, Boltz, Borzoi, Chai-1,
  DiffDock, ESM-2, ESMFold2, Evo 2, LigandMPNN, ProteinMPNN, SolubleMPNN,
  OpenFold3, scGPT, Deep Research, Literature Review, Indication Dossier.
- Not applicable to Sandhill, because they drive parts of Claude Science
  itself: Built In Browser, Chrome Browser, Computer Use, Docs, Google
  Workspace, Import Memory, Morning.

So the honest starting list is the eight above, one of which is in the
screenshot. The rest can come two ways, both outside this plan:
1. Anthropic publishes them; Sandhill's list then gains them with no new
   code.
2. Sandhill gets its own skills for those tools, written from the tools'
   public documentation (as its four bioinformatics guides were). That is
   writing work that needs Huan's review of each one.

Anyone can still add any skill they have the right to use, from GitHub, a
folder or a zip file.

## What gets built

### 1. Where skills live
On each device, in Sandhill's own state folder: `skills/<name>/` holds the
skill's files unchanged, and `skills/index.json` records for each one its
name, description, where it came from (list, GitHub address and commit,
or upload), its licence file if any, whether it is on, and when it was
installed. Sandhill never writes into the agents' own folders
(`~/.claude`, `~/.codex` and the rest), as settled in
`docs/decisions/agents.md`.

### 2. How every agent gets them
The same way the four built-in guides reach every agent today, so one
install works for all seven agents:
- Sandhill's tool server gains one tool, `get_skill`. With no argument it
  lists the skills that are on (name and description). With a name it
  returns the `SKILL.md` text, the folder's path on the device and its
  file list, so the agent can read the reference files and run the scripts
  from that folder. With a name and `file`, it returns that text file of
  the skill, for agents whose own sandbox can't read the folder.
- The instructions Sandhill already gives each agent gain one sentence:
  "Installed skills are listed by `get_skill`; read a skill before work
  that matches its description."
- "/" in the message box lists installed skills that are on, under
  Sandhill's own section next to the guides (sent by the daemon as guide
  items, so the message box needs no change). Picking one makes a chip and
  sends "For this message, follow the skill X: read it first with the
  `get_skill` tool."

Rejected: handing skills to Claude Code through its own `--plugin-dir`
option. It works only for Claude Code and would give two different
behaviours across agents.

### 3. Installing
A skill can come from three places:
- **Sandhill's list** ("Suggested"): the eight skills above, each pinned
  to an exact GitHub commit in a list built into the daemon
  (`crates/daemon/assets/skills/catalog.json`). They are downloaded when
  Huan presses Add, not shipped inside Sandhill.app.
- **GitHub**: a repository address, optionally with a folder inside it.
  The device fetches it with `git` (already needed by the Git pane), so
  private repositories work with the git sign-in the device already has.
  If the repository holds several skills, a list with checkboxes chooses
  which.
- **A folder or zip file** on the Mac, sent to the device.

Before anything is installed, a card shows: name, description, where it
comes from, its licence, the list of files, and a warning line when it
contains scripts. Buttons: Install, Cancel.

Limits, checked on the device before installing: must contain `SKILL.md`
with a name and description; at most 20 MB and 2,048 files per skill; no
links to other places (symlinks) and no paths that lead outside the
skill's folder; a name already in use asks "Replace?".

If a server has no internet, install on This Mac and use Copy to….

### 4. The Skills page
A new "Skills" row in Welcome's sidebar, above Usage, with the device
picker the Usage page uses.

- **Built in**: the four bioinformatics guides, listed with their
  descriptions (no switch; they stay as they are).
- **Installed**: each with a switch (on/off), its source in grey, and a ⋯
  menu: Show Files (the same card as before installing, with the file list),
  Check for Updates… (GitHub and list skills only), Copy to… (lists the
  other connected devices), Remove.
- **Suggested**: the list's skills not yet installed, each with its
  description, licence and an Add button.
- **Add Skill** button, top right, a menu: From GitHub…, From a Folder or
  Zip….

Check for Updates is manual, as in Claude Science: it fetches the skill's
repository at its newest commit and shows the confirm card with Update
when the skill's files changed, or "Up to date" when they did not.

Copy to… sends the skill's files and its source (so updates still work
there) and installs without a card, unless a skill of that name is already
on the other device, which asks to replace it.

The page follows `docs/decisions/look.md`: the switch, capsules, menus and
cards already in use; no new styles.

### 5. Daemon calls
`ListSkills`, `PreviewSkills {fetch}` (fetches into `skills/.incoming/`
and returns what the card shows, under a token), `InstallSkills {token,
names}`, `SetSkillEnabled {name, enabled}`, `RemoveSkill {name}`,
`ExportSkill {name}` (files and source, for Copy to…) and `GetSkill {name?,
file?}` (for `get_skill`). A fetch is a git address, a Suggested id, an
installed skill's update, a zip file or a folder's files. Types in
`crates/proto/src/skills.rs`; added to `docs/protocol.md`.

## Not building (in this plan)
- Connectors (next plan).
- Taking anything out of Claude Science.app.
- Writing a skill inside Sandhill, or "create a skill by chatting".
- Automatic updates, or any background checking.
- Per-project skills: skills apply to every project on the device.
- Claude Science's Specialists and Memory.

## Order of work
Each step leaves Sandhill usable.
1. **Daemon:** the store, the limits, install from a folder, zip or
   GitHub, on/off, remove, `get_skill`, the "/" list. Tested in
   `crates/daemon/tests/skills.rs`.
2. **App:** the Skills page with Installed, Built in, Add Skill and the
   confirm card.
3. **List and upkeep:** Suggested with the eight skills, Check for
   Updates…, Copy to….

## Tests
In `crates/daemon/tests/skills.rs`, using `common::tempdir()`:
- Install from a folder and from a zip; the files arrive unchanged.
- Refused: no `SKILL.md`; no name or description; a symlink; a path
  leading outside; too large; too many files.
- Same name: asks to replace, and Replace swaps the files.
- Off: missing from `get_skill` and from "/"; on again: back.
- Remove deletes the folder and the record.
- `get_skill` through Sandhill's real tool server returns the text, path
  and file list.
- GitHub: a local git repository stands in for GitHub (one skill, several
  skills, a pinned commit); update check finds a changed skill and leaves
  an unchanged one alone.
- Export from one state folder and install into another gives the same
  files.

These need no kernel, so the "real kernel" rule for daemon tests does not
apply here; the tool-server test uses the real `sandhilld mcp`.

In the app, checked on screen in light and dark: the page, the confirm
card, the menus, a skill picked from "/".

## Risks
- **A skill is instructions an agent will follow, and may carry scripts.**
  The confirm card shows the source, licence and files first, and nothing
  is ever installed or updated without a click. Agents' own approval cards
  still apply when a skill's script is run.
- **This plan assumes agents will call `get_skill` when a task matches.**
  The built-in guides rely on the same thing. If an agent ignores it, the
  "/" pick still forces the skill for that message.
- **Licences of user-added skills** are the user's responsibility; Sandhill
  shows the licence file when there is one and "No licence file" when not.
  Only the eight listed skills are offered by Sandhill itself, all
  Apache-2.0, and each keeps its `LICENSE` file when installed.
- **Rollback:** removing the `skills/` folder on a device returns it to
  today's behaviour; nothing else on the device is touched.

## Huan's decisions (2026-11-16)
1. The eight-skill starting list is fine.
2. No Sandhill-written skills for the other tools for now; Huan will ask
   when they are needed.
