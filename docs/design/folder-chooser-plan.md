# Folder chooser plan (Huan, 2026-11-14, approved)

Huan asked whether Open Folder could use the system's Finder window, and
whether the built-in chooser could do more. He approved all three parts.

## This Mac: the system window

When the folder is on this Mac, Open Folder… (Welcome, ⌘O, ⇧⌘N, the ☰ menu,
the command palette) and the plot window's Choose… use the system's folder
window (`cx.prompt_for_paths`, folders only). It brings Finder's sidebar,
iCloud, recents, search, tags and New Folder. The chosen path is handed to the
daemon as before (`OpenProject`), so the app still never opens a project or
file itself; the daemon refuses anything that is not a folder.

## Servers: the built-in chooser, extended

Servers keep `folder_dialog.rs`, which lists folders through the server's
daemon. It gains:

- A sidebar of places: Home, `/`, and the folders that hold the device's
  recent projects (the plot chooser adds the project folder).
- A clickable path along the top (`/ › home › huan › data`); long paths keep
  the last segments.
- One field to filter the list by name, or to type a path (starting with `/`
  or `~`) and press Return to go there.
- Keyboard: ↑/↓ select a folder, Return or ⌘↓ enter it, ⌘↑ goes up, ⌘Return
  opens, Escape clears the filter and then closes. ⌥⌘N makes a new folder
  (Sandhill's New Folder shortcut; ⇧⌘N stays New Project).
- A single click enters a folder; a double click opens it straight away.
- New Folder (named in place, then selected) and a show/hide toggle for
  names starting with a dot.
- Files are listed after folders, faded and not clickable, to recognise
  the right folder.
- Folders that are Git repositories show the Git mark; folders with a
  `.sandhill` folder show the project folder icon. The daemon reports both
  when `ListDir` asks for `marks`; older servers just show no marks.
- Open opens the selected folder, otherwise the folder being shown; the
  footer says which.

## Drag a folder onto Welcome

Folders dragged from Finder onto Welcome open as projects on this Mac,
one capsule each, whichever device is selected. Files are refused with a
short message (the daemon reports them as not folders).
