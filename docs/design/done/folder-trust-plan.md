# Plan: trusting a project folder

Status: approved by Huan on 2026-11-12 with every recommended choice:
folders already in a device's project list are trusted automatically; the
three agents that cannot be told per run keep their own question, answered
by the user; Restricted Mode has no Terminal. Earlier the same day Huan
decided to build it and that Sandhill never edits an agent's configuration
file for this.

Implemented 2026-11-12. What building it settled:
- Trust is a `trusted_folders` table in the daemon's journal. The upgrade
  that adds it trusts every project already in the device's list.
- Only folders opened as projects are held back; a path outside every
  project works as before (the daemon's own callers, a Console elsewhere).
- Refused calls: `start_session`, `lsp_start`, `start_terminal`, `start_job`,
  `restart_terminal`, `preview_document`, `suggest_cell` and every `git_*`
  call that runs Git, with an error starting "Restricted Mode:". Reading,
  editing, saved outputs and file previews work.
- Restricted Mode also shows no Variables, Kernels, Help or Packages, and
  does not attach to kernels already running in the folder.
- "Open in Restricted Mode" is kept while the window stays open (also across
  a reconnect); the next open of an untrusted folder asks again.
- Codex: confirmed with the real CLI. The whole `projects` table is passed
  as one `-c` value, as a dotted key cannot hold a path with dots or spaces.
  pi: confirmed with the real CLI through the `project_trust` event; nothing
  is saved in pi's own trust file.
- Removing a folder from Trusted Folders while its project is open leaves
  that window as it is; the daemon refuses new starts and the next open asks.

## Why

Opening a project runs nothing, but two later steps act on what the folder
holds: starting R or Python (`.Rprofile`, `.Renviron`, the project's env
files, the kernel chosen in `.sandhill/project.toml`) and starting an agent
(`CLAUDE.md`, `AGENTS.md`, `.mcp.json`, hooks, project-local extensions).
A cloned repository, a folder someone sent, or a folder others can write to
on a shared server can therefore run code as the user. Today each agent asks
about this separately; Sandhill asks once instead.

## What the user sees

- **First open of a folder**: a centred dialog over the project window,
  "Do you trust the files in <name>?", with the device and full path, one
  line on what trusting allows, and two buttons: **Trust Folder** (default)
  and **Open in Restricted Mode**. No second page and no feature comparison.
- **Trusted**: remembered for that device and folder, and for folders inside
  it. Never asked again.
- **Restricted Mode**: files, saved outputs, previews and the editor work.
  Kernels (Console, running cells, Variables, Help, Packages), Terminal,
  Jobs, Git, language servers and the AI pane do not start; each shows one
  muted line, "Restricted Mode: trust this folder to use …", and the status
  bar shows a "Restricted" chip that opens the same dialog. Git is included
  because a repository's own configuration can run programs.
- **Managing**: Welcome → Manage gains "Trusted Folders": device, path and a
  remove button per row, for the connected devices. Removing asks again on
  the next open; it does not stop anything already running.
- Trust is about the folder's files only. Agent approval cards and modes are
  unchanged.

## Where it lives

In the daemon, on the device that holds the folder, so local and remote work
the same way and the rule holds for every client.

- State: a `trusted_folders` list in the daemon's state directory (canonical
  paths). Not in the project's `.sandhill` folder, which the folder's author
  controls.
- Protocol: `OpenProject`'s `ProjectInfo` gains `trusted`; new calls
  `TrustFolder { path }`, `UntrustFolder { path }`, `ListTrustedFolders`.
- Enforcement: calls that start a process for a project (sessions, terminals,
  jobs, agents, language servers, Git) are refused with a clear error while
  its folder is untrusted. The app hides the entry points; the daemon is what
  guarantees it. Kernels already running in a folder keep running and are
  attached to only after trust.
- Older daemons report no `trusted` field; the app treats that as trusted, as
  today, until the server daemon is updated.

## The agents

Sandhill tells each agent per run where the agent has a way to be told, and
never edits an agent's configuration. Checked on this Mac on 2026-11-12 from
each CLI's help, documentation or program text; none was run for this.

| Agent | Its own question | Per-run way | Result |
|---|---|---|---|
| Codex 0.167 | TUI asks; saved as `projects."<path>".trust_level` | `-c` overrides any config value | Not asked. To confirm while building that `-c` accepts the quoted path key. |
| pi 1.5.2 | Asks when the folder has project-local resources | `--approve`; or the `project_trust` event in an extension | Not asked: Sandhill's per-run extension answers it. |
| Oh My Pi 18.19 | None found | — | Nothing to do. |
| OpenScience 2.0 | None found (chat only) | — | Nothing to do. |
| Claude Code 2.1 | TUI asks once per folder; skipped only in `-p` mode, which Sandhill's chat does not use | None | Its question stays, as the chat card Sandhill already shows. |
| Posit Assistant 0.1 | TUI asks "Trust this workspace?" | None found | Its question stays, on its own screen. |
| Antigravity 1.5 | TUI has "Trust this folder?"; not known whether headless chat asks | None found | Its question stays where it appears. |

So after trusting a folder in Sandhill: four agents never ask, three may ask
once more each, the first time they are used in that folder.

## Tests

- Daemon integration test with a real kernel: an untrusted folder refuses a
  session, terminal and job; after `TrustFolder` they start; a subfolder of a
  trusted folder is trusted; removal is asked again.
- Real `pi` (temp HOME) with a project-local extension: starts without its
  question in a trusted folder. Real `codex`: the override is accepted.
  Both skip with a message when the CLI is missing.

## Questions for Huan

1. Folders already in a device's project list when this arrives: trust them
   automatically (recommended; they were opened and used before), or ask
   for each on its next open?
2. For the three agents whose question stays: leave it for the user to
   answer (recommended, as decided), or have Sandhill answer "yes" in the
   agent's own question for a folder already trusted in Sandhill? The second
   edits no file, but the agent then records the trust itself, on Sandhill's
   say-so.
3. Restricted Mode as listed (no kernels, Terminal, Jobs, Git, language
   servers, agents), or keep Terminal available?
