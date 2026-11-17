# Answering code that asks a question (plan, approved 2026-11-17)

Huan approved this on 2026-11-17 ("Yes, fix it").

## The problem
Sandhill ran every request with `allow_stdin: false` and never listened on
the kernel's stdin channel, so code that asks for input could not be
answered. Tested on 2026-11-17 against a separate daemon:
- R `browser()` hung the session: Ark asks on the stdin channel even when
  input is not allowed, and nobody answered. `readline()` and `menu()`, and
  so package installs that ask "Update all/some/none?", do the same.
- Python `input()`, `getpass()`, `pdb.set_trace()` and `%debug` failed with
  `StdinNotImplementedError`.
- Python `breakpoint()` was silently skipped: ipykernel 6+ points
  `sys.breakpointhook` at debugpy, which does nothing unless a visual
  debugger is attached.

With input allowed, Ark asks through the stdin channel with the prompt
(`Browse[1]> `, `Selection: `, `name? `), and ipykernel does the same for
`input()`, `getpass()` (marked as a password) and pdb (`ipdb> `).

## Daemon
- The kernel connection also opens the stdin socket, with the shell
  socket's identity (the protocol requires it, so the kernel can route its
  question to us).
- Runs ask with `allow_stdin: true`; quiet runs (tools looking around) keep
  `false`. A question from a quiet run is answered by interrupting it, so a
  tool can never leave a session waiting.
- A question becomes a journaled `sandhill.input_request` event on its run:
  `{prompt, password, header}`. `header` is the kernel's request header, kept
  so that a daemon restarted while the kernel waits can still answer.
- `Call::Input {session, value}` answers the session's question. The daemon
  journals the prompt and the answer as the run's stdout (`Browse[1]> n`;
  a password shows as the prompt alone), then `sandhill.input_reply`, then
  sends the reply. It fails when nothing is being asked. The first answer
  wins when several windows show the question.
- Python sessions start with `sys.breakpointhook` back at Python's own, so
  `breakpoint()` stops in the debugger (`PYTHONBREAKPOINT` still applies).
- Agents: an agent's run that asks a question stops being waited for, and
  the agent is told the code is waiting for the user's answer in the
  Console. Agents do not answer questions themselves.

## App
- A run that is asking shows its prompt at the end of its output in the
  Console, in the accent colour, with "answer below".
- While the Console's session is asking, Enter in the Console input sends
  the text, empty included (an empty line means something in `browser()`),
  as the answer instead of running it as code. `?topic` and history
  recording are skipped for answers. A password question hides what is
  typed.
- A cell run that asks a question brings the Console tab forward; running a
  cell already moves the Console to the document's kernel.
- Interrupt still works while a question is open.

## Tests
Daemon integration tests with real kernels (skipped when missing): Python
`input()` answered; `getpass()` answer kept out of the journal;
`breakpoint()` stops and continues; R `browser()` answered step by step;
a quiet run that asks is interrupted, not left waiting; answering when
nothing is asked fails; Interrupt ends a run waiting for input; an agent's
run that asks reports it.

## Built (2026-11-17)
Decisions made while building:
- The prompt also shows above the Console input, with what Enter does,
  since a notebook cell's runs can be left out of the Console.
- A password question puts a one-line hidden field in place of the Console
  input (the code editor can't hide text), and focus moves to it and back.
- After a daemon restart the open question is found again from the
  journal (`open_question` in `session.rs`), and the app rebuilds it from
  the replayed events (`Run::question_from_events`).
- Tests: `crates/daemon/tests/input.rs` (6), an agent case in
  `crates/daemon/tests/mcp.rs`, and the reopened Console in
  `app/src/workbench/console.rs`.
