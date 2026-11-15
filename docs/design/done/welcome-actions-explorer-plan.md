# Welcome project actions and data explorer plan (Huan, 2026-11-14; approved)

## 1. Project rows on Welcome

Every project row (a device's page and the Projects page) gets:

- **Save All Files**: saves the project's edited files. Only for an open
  project with unsaved edits.
- **Stop All Kernels**: shuts down the project's kernels on its device,
  after a confirmation naming how many ("Variables in them are lost").
  Agents keep running.
- **Close Project**: replaces today's "Close the tab?" question.
  - Nothing unsaved and no running kernels: it closes at once.
  - Otherwise one question lists what is open ("2 unsaved files, 3 running
    kernels"), with three choices:
    - **Save and Close**: kernels keep running.
    - **Save, Stop Kernels and Close**.
    - **Cancel**.
  - A file that fails to save keeps the project open, as now.
- **Remove from list (×)**: an open project asks "Close proj and remove it
  from the list?" first. Closing then follows Close Project above. A project
  that isn't open is removed at once, as now. Kernels running there aren't
  stopped by removing it.

## 3. Data explorer

- **Open in a new window**: a toolbar button moves the explorer into a
  window of its own. Its tab leaves the main window, and closing the window
  closes the explorer.
- **Column headers**:
  - The type line under each name goes.
  - A **⋮** button on each header replaces the sort icon. Its menu, after
    Positron:
    - Copy Column (⌘C)
    - Select Column
    - Pin Column
    - Sort Ascending
    - Sort Descending
    - Clear Sorting
    - Add Filter
  - A sorted column keeps a small arrow beside its name.
- **Toolbar**:
  - A gear button, icon only, with a menu:
    - **Show summary panel by default** (a tick, remembered on this Mac)
    - **Reset view**: clears filters, sorting, hidden and pinned columns,
      and column widths.
  - RStudio's "filter UI" and "overlay scrollbars" have no counterpart here:
    the filter chips are always in the toolbar and scrollbars always overlay.
  - The search field starts with a magnifier icon.
  - A **Copy filters as code** button, greyed out when there are no
    filters.
- **Summary panel (left)**:
  - Each column shows its missing share ("3.2% missing"). Hovering it shows
    the count ("12 of 375 values missing").
  - The statistics now at the bottom move under each column's row, folded
    like folders in Files: its chevron opens and closes them, and several
    can be open.
  - Clicking a column's name scrolls the table to that column and selects
    it.

## Huan's answers

1. Row actions: a **⋯** button on each row (and right-click) with the
   actions.
2. Close Project's default (Return): Save and Close, kernels keep running.
3. Pin Column: built now.
4. **Copy as code** (the whole view) stays beside **Copy filters as code**.
