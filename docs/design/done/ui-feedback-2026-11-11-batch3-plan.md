# UI feedback, 2026-11-11 (third batch): plan for the larger items

Status: implemented 2026-11-11. Approved by Huan the same day, with the dpi made editable (item 9),
Resources and storage… below the panel's right edge, Save always drawing
afresh, and headers at 82% opacity. Small fixes from the same batch (bold
project names, the model popup, the chat/TUI switch, notebook line numbers,
centred add-cell buttons, the Quarto `{r}` template) were done directly and
are not in this plan.

## 1. Welcome screen

One panel, as now, with the controls moved to where they act:

```
 [crane] Sandhill
         R and Python notebooks that keep running when you close the lid.

 ┌──────────────────────────────────────────────────────────────┐
 │ [laptop This Mac ●] [server lab ●]        [+ Connect to Server…] │  tab bar
 ├──────────────────────────────────────────────────────────────┤
 │ Projects                                     [folder Open Folder…] │  section row
 │  proj-a                                          opened today │
 │  proj-b                                     opened 3 days ago │  scrolls inside
 └──────────────────────────────────────────────────────────────┘
                                         [gauge Resources and storage…]
 Kernels keep running after you close a project…
```

- Device tabs grow from the editor tab size to 34px tall, text 15px
  (`look::CHROME` + 2), icons 15px, the status dot after the name as now.
- Buttons share one style family so they read as a set: 28px tall,
  `look::RADIUS`, 13px text, a 14px Lucide icon then the label.
  Open Folder… is the one filled (brand accent) button; Connect to Server…
  and Resources and storage… are quiet outline buttons on the glass.
- Open Folder… sits at the right end of the "Projects" row and acts on the
  selected device tab. Connect to Server… sits at the right end of the tab
  bar. Resources and storage… sits below the panel, right-aligned with it.
- Both open a centred modal dialog instead of the inline cards:
  - Open Folder: title "Open Folder on This Mac" (or the server name), the
    existing path field, Up/Home, the folder list, and Cancel / Open. On this
    Mac it stays Sandhill's own picker (the daemon reads the folders), so
    local and remote behave the same.
  - Connect to Server: the server field with its hint, Cancel / Connect;
    connection errors show inside the dialog and keep it open.
  - Escape and Cancel close; clicking outside closes.
- The inline `add_server_card`/`picker_card` are removed.

## 3. Translucent tab bars and toolbars

What GPUI can do: the window blur is one macOS effect behind the whole
window. GPUI has no per-element blur, so a toolbar cannot blur what scrolls
under it; getting that would mean changing the drawing engine. The plan is
the closest effect without that:

- A pane's tab strip and its tool rows (Files' folder row, the filter rows of
  Variables/Packages/Help, the editor action row) float over the pane's
  content as one header block with the pane's own colour at about 82%
  opacity and its hairline bottom edge. Scrolled content passes under it and
  shows faintly; at rest nothing is hidden, because the content starts below
  the header.
- Scrollbars start below the header. "Scroll to" (jump to cell, reveal a
  file, find next) leaves the target below the header, not under it.
- Applies to: notebooks and Quarto notebooks, Console output, AI chat and
  history, Variables, Kernels, Help, Packages, Files, Git lists, Jobs list.
- Not applied: terminals (their rows are sized to the box, so the top rows
  would be hidden for good), Plots and Viewer (nothing scrolls; the Viewer's
  web page is a native view that always draws on top), and the script/Source
  editor (Kit's editor scrolls inside itself and has no top inset; it would
  need a change to Kit's editor, which can come later).


## 9. Enlarged image window

One window for every enlarged picture: the Plots pane's magnifier, and a new
magnifier on notebook (and Console) image outputs, beside their Save and Copy
buttons. It is today's plot window (`plots.rs`), extended:

```
 ┌ plot.py · 1 of 3 · just now ─────────────────── ←  →  × ┐  drag here to move
 │                                                          │
 │                     (the picture)                         │
 │                                                          │
 │ PNG SVG PDF                                               │
 │ Width [ 6.40 ]  Height [ 4.80 ]  [in ▾]   at [300] dpi    │
 │ (●) Size follows window    (●) Redraw                     │
 │ Folder [ plots/            ] [Choose…]  Name [ plot.png ] [Save] │
 └──────────────────────────────────────────────────────── ◢ ┘
```

- **Opens at the same size.** The picture first shows at exactly the size it
  had in the pane or under the cell; the window is built around it (wide
  enough for the controls) and centred.
- **Moves.** Dragging the title row moves the window anywhere in the app
  window; the corner still resizes it. Position and size last while it is
  open.
- **Size follows window** (on by default): resizing the window changes the
  export size so the file matches what is shown. The rule is the one the
  preview already uses, 96 screen points per inch: a picture shown 614×461
  points exports as 6.40×4.80 in, which is 1920×1440 px at 300 dpi. Typing a
  size turns the switch off, so a typed size is never overwritten.
- **Units:** px, in or cm, labelled Width and Height, and an editable dpi
  field (Huan), which links px to in/cm. Changing the unit converts the
  numbers, never the size; changing dpi keeps the physical size (in/cm) and
  changes the pixel count.
- **Redraw** (on by default): resizing draws the plot again at the new size
  through its live R/Python session (`RedrawPlot`), as now. Off: the
  picture is only scaled, like any image, and nothing is sent to the
  session. Plots that cannot be redrawn (session ended, not matplotlib/R,
  past the last 10 kept) show the switch off and disabled, with the reason
  in its tooltip.
- **Folder:** a Choose… button opens a folder dialog on the project's
  machine (the same dialog as Welcome's Open Folder, item 1). The default is
  `plots/` at the project root, created on Save if missing (today it is
  `figures/`). The last folder used is remembered per project across
  restarts and used the next time the window opens.

- **Saving with Redraw off** (Huan): Save still draws the plot afresh at the
  export size whenever its session can; Redraw only affects the preview.
  Plots that cannot be redrawn save their original bytes, as today.
