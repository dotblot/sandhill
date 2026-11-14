# Plan: code completion to RStudio's standard, and Console history

Status: approved by Huan on 2026-11-13, with plain words completing from
their first letter (not the second) and item 1 meaning: only "Use for new
chats" changes the default account.

Huan:
1. Default accounts: only the quota panel's "Use for new chats" sets them.
2. In an empty Console input, Up recalls the last command; Up/Down then walk
   the history; once the recalled command is edited, Up/Down move the cursor.
3. Code completion in the Console input too.
4. R completion lists unrelated names (`libr` offers `palette`,
   `packageDate`…), and inside `getKEGG(org|)` it offers unrelated names
   instead of the arguments. It should reach RStudio's standard.
5. Python: after `.` the list is slow, vanishes at the first letter and
   returns only at the third, though one often doesn't remember the name.
   Arguments inside a method's parentheses have the same problem.

## Why it happens today (`gpui/src/language/providers.rs`, Kit 0.9.1)

- Completion asks Ark's language server for R (the kernel is skipped for Ark)
  and both the live kernel and `ty` for Python, then shows the merged list
  in the order received. Nothing filters or sorts the list by what is typed:
  neither Sandhill nor Kit uses the typed prefix, `sortText` or
  `filterText`, so Ark's first items (any attached function) show first.
- A request starts only after `$ @ . [ ::` or the third letter of a word.
  Kit closes the list on every keystroke, so after `.a` and `.ab` nothing
  asks again until `.abc`.
- `(` and `,` start nothing, so arguments are offered only after three
  letters, and argument items whose replacement range reaches outside the
  current token are dropped while mapping notebook cells
  (`map_completion`).
- Python waits for the kernel's answer (jedi inside ipykernel can be slow)
  before showing `ty`'s.
- Signature help (the argument hint inside parentheses) does not exist.
- The Console input has no Up/Down history; for Ark, the Console has no
  completion at all (no document for the language server, kernel skipped).

## Change

### A. Accounts
Switching a conversation's account no longer changes the default for new
conversations; only "Use for new chats" in the quota panel does.

### B. Ranking and filtering (both languages, editor and Console)
Sandhill filters and orders the list itself before Kit shows it:
1. Items matching the typed word: exact-case prefix, then any-case prefix,
   then word-start matches (`rN` → `rowNames`), then subsequence matches;
   items that don't match are dropped.
2. Inside a call, the call's arguments (`org =`) come first, as in RStudio;
   after `library(`/`require(`, installed package names.
3. Ties keep the server's `sortText`, then shorter names first.
4. The list asks again on every letter after a trigger and from the first
   letter of a plain word; Tab always opens it. Old answers are never shown
   for newer text.

### C. Arguments and signature help
- `(` and `,` inside a call start completion of argument names (Ark and
  `ty` both return them; the kernel too for Python).
- Argument items are no longer dropped by the notebook-cell mapping.
- A small hint above the cursor shows the function's signature with the
  current argument in bold while the cursor is inside the parentheses
  (`textDocument/signatureHelp` from Ark/`ty`; the kernel's `inspect` when
  no server answers), 13px like other popups, closed with Escape or `)`.

### D. Python speed
The kernel and `ty` are asked together; whichever answers first is shown,
and the kernel's answer is waited for at most 300 ms after `ty`'s.

### E. Console
- History: as Huan described. Up in an empty input recalls the newest
  command; Up/Down walk older/newer while the text is an unedited recalled
  command; Down past the newest empties the input; after any edit Up/Down
  move the cursor. History is the session's (already kept by the daemon).
- Completion: R (Ark) and Python ask the live kernel, with the same ranking,
  arguments and signature hint as the editor.

## Checks
- Unit tests: ranking order (prefix, case, word start, arguments first),
  the trigger rule, the history walk (recall, walk, edit stops it, Down
  past newest), argument items kept by the cell mapping.
- In the app with real Ark and Python kernels: `libr` → `library` first;
  `getKEGG(` → its arguments first and a signature hint; `np.` → list at
  once, narrowing with each letter; the Console cases above.

## Implemented: Console history (E)
`gpui/src/console.rs`: `history_step` (pure walk) and `history_codes` (the
Console session's commands newest first, consecutive duplicates dropped);
`Workbench::console_history_step` applies a step, keeping the recalled
position and text in `Sessions::recall`, and puts the cursor at the end.
"Unedited" means the input still equals the recalled text, so any edit stops
the walk and an emptied input (including after running a command) restarts
it. `gpui/src/view.rs` captures `MoveUp`/`MoveDown` on the Console input's
wrapper and lets them through when the step does not apply or Kit's
completion menu is open (`completion_menu_state().open`). Up on the oldest
command stays there. Tests: `console_history_walk_*`.

## Implemented: ranking, arguments, signature hint, Python speed, Console completion (B, C, D, E)
`gpui/src/language/providers.rs`: `rank` filters and orders every answer
before Kit sees it. Each item is matched on its `filterText` (else label)
against the text its own replacement covers up to the cursor (else the typed
name; R names include dots): exact-case prefix, any-case prefix, word starts
(`match_tier`, `abbreviates`), subsequence; non-matches are dropped. Inside a
call (`signature::call_site`), items reading `name =`/`name=` lead the
prefix group and the looser group; after `library(`/`require(`/
`requireNamespace(` package items (`kind` Module) do. Ties: `sortText`
(label when absent), then length. Duplicates by resulting text keep the
language server's item (it has kind and docs) over the kernel's. The trigger
(`trigger`) fires on `$ @ . [ :: ( ,` and every letter of a name from the
first, not in numbers (`1.5`) or comments. Tab (`complete_on_tab`, captured
`IndentInline` in `view.rs`) dispatches Kit's `show_completions` after a
name, `$ . :: ( , [ @`; elsewhere Tab indents. Kit already drops answers
for older text (document revision); unchanged. `map_completion` now keeps a
server item whose edit leaves the cell but spans the cursor, replacing the
typed name instead. Python asks the kernel and `ty` together and waits for
the kernel at most 300 ms after `ty` answers (fully when there is no server
answer). The Console: Python uses the kernel; Ark answers
`complete_request` with nothing, so the Ark Console input goes to Ark's
language server as its own one-cell document
(`<project>/.sandhill/console.sandhill.r.R`, same session host). When Ark
gives no packages in `library(` (IRkernel), the session's `list_packages`
supplies them (1.5 s bound).
`gpui/src/language/signature.rs`: the hint. A Kit observer follows each
editor; `(` or `,` before the cursor asks `textDocument/signatureHelp`
(Ark, `ty`), else the kernel's new `inspect` call (IPython `Signature:`, R
`function (…)`), retried briefly when a completion holds the kernel. It is
asked again when the argument or its `name =` changes; leaving the call,
`)` closing it, a selection or blur hides it; Escape hides it for that call.
A deferred element places it above the caret from Kit's current-frame caret
geometry (below when no room), 13px monospace, current parameter bold.
Daemon: `Call::Inspect` → `{found, data}` (`docs/protocol.md`), integration
test `ipykernel_inspects_live_function_signature_without_a_run`. Kit still
closes its menu on every keystroke until the next answer arrives (a short
flicker); Tab does not accept a suggestion (Kit has no public accept action).
Tests: `ranking_filters_and_orders_by_match`,
`arguments_and_packages_come_first`, `duplicate_results_keep_the_server_item`,
`cell_mapping_keeps_items_spanning_the_cursor`, `automatic_trigger_rules`,
`signature::tests::*`.

## Addendum 2026-11-13: accepting calls, typing pause, hint placement
`gpui/src/language/providers.rs`, after `rank`, `finish` turns every item
into plain text: a snippet (Ark's `library($0)`, `ggplot2::ggplot($0)`)
loses its syntax (`snippet_text`: `$N`, `${N}`, `${N:default}` nested,
`${N|a,b|}` → first choice, variables → their default, `\$ \} \\`
escapes); a Python FUNCTION/METHOD/CONSTRUCTOR item (`ty`, or the kernel's
`_jupyter_types_experimental` `type` = `function`) gains `()` unless the
line is an `import`/`from`/`def`/`class`/decorator line or the next
character continues a name; before an existing `(` neither keeps `()`. R
relies on Ark's snippets (no kind-based parentheses). `coalesce_edits`
escapes the plain text it folds around a snippet primary. Duplicates are
judged on the plain text without a trailing `()`, so Ark's `library()`
absorbs the kernel's `library`. Kit's `insert_completion` puts the cursor
at the end of the inserted text and has no post-accept hook, so each item
whose cursor belongs inside is remembered (`Accept`: replaced range, text,
cursor; the last 512 kept across answers); the editor's `Change` handler
recognises the edit that is exactly Kit inserting one of them (old/new
lengths, inserted text, cursor at its end) and moves the cursor to `$0`
(else the lowest tab stop; inside the parentheses).
Typing pause (Huan's rule): the 1st and 2nd letters of a plain word ask
for completions only after 3 s with text and cursor unchanged (`pauses`,
`TYPING_PAUSE`); from the 3rd letter, immediately. Letters of a word right
after `. $ @ :: ( , [` (spaces allowed) are immediate, so `np.l` keeps
refining; an open menu refines with every letter. Any keystroke or request
(including Tab's) bumps a generation that cancels the wait; the wait ends
in Kit's `show_completions` (Tab's entry point) in the window where the
editor has focus. When the first request comes mid-word (3rd letter) it
also goes through `show_completions` (next tick) so Kit's query starts at
the word, not at that letter.
`gpui/src/language/signature.rs`: the hint is built in its deferred
prepaint at its text's shaped width (max 640px, window − 16px), so a
wrapped signature is measured at the width it paints at; its bottom sits
4px above the caret line (growing upward), else below the line
(`hint_top`). While Kit's completion menu is open the hint stays above, and
hides when there is no room above or the menu may open above the caret
(room below < 256px = list 240 + popover chrome + gap, and more room above;
the rule in the vendored `caret_anchored.rs`).
Tests: `snippets_become_text_with_the_cursor_at_their_stop`,
`calls_get_parentheses_with_the_cursor_inside`,
`first_letters_of_a_plain_word_wait_for_a_pause`,
`signature::tests::hint_sits_above_the_caret_line_and_avoids_the_menu`.
