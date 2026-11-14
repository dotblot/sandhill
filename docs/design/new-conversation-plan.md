# The AI pane's new conversation

Approved by Huan on 2026-11-14 ("按推荐"), after a mockup.

## Why
The screen was three grey paragraphs over empty space: no sign it is where
a conversation starts, the chosen agent and account in one small grey line,
two paragraphs saying one thing, and nothing to start from.

## What it shows, top to bottom
1. **The agent**, centred as the pane's "No conversations" state is: its
   mark in the 44px accent tile, its name, then "model · account" and one
   line: "Works on all of {project} and its live sessions."
2. **The live sessions** of the project as small round chips with the
   kernel's dot (green idle, blue running): what "its live sessions" means.
3. **Start with**: up to three rows. Clicking one fills the message box and
   its chips; nothing is sent. They are built from the project:
   - "Explain what this project does", always;
   - "Look over [file] for problems", the document open in the editor (else
     the last tab), attached as an @ file chip;
   - "Summarise [table]: its columns, missing values and anything odd", the
     first data frame or matrix in a live session, attached as an @
     variable chip;
   - with neither, "Suggest what to do next in this analysis".
   What goes with the message is only what the chips show, as for anything
   added with @ and #.
4. **Continue**: the three most recent conversations of the project, agent
   mark, title and time; clicking one opens it. Hidden when there are none.
5. A line of hints: `@` attach a file, `#` attach a cell, `⏎` send.

The agent missing on this device keeps its install note; agents that open
their own screen keep one line saying so. The message box, model picker and
tab bar are unchanged.

## How
`agents_start.rs` draws it and reads the context: the project's running
sessions (`ListSessions`), the first one's variables (`ListVariables`,
asked with a time limit so a busy kernel is skipped), and the workbench's
open document. It is read when the screen shows and again after 15 s.
