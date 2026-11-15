# Data explorer redesign

Huan, 2026-11-14. Status: approved and built, with the shown checkbox moved
after the missing share (Huan).

## What is wrong

Seen in the app with a 2,000 × 6 pandas table:

- **One crowded action row.** Twelve things share it in no clear order:
  the column panel toggle, a sentence ("Python · df, live"), Read again,
  two long text buttons for code (one usually dimmed), the filter chips,
  Add filter, Search, then Export, Open in a new window and Settings as bare
  icons. With a few filters the chips are cut short; in a narrow pane the
  table's name is cut first.
- **The table's size is only in the footer**, and its state is a word
  ("live", "running code…") in the middle of the sentence.
- **The column panel has no headings.** "Show all"/"Hide all" float above
  the list. Each row holds a fold arrow, a ☑ text character, the name over
  a cut-off type line ("float64 · 120 miss…"), a sparkline and an
  unlabelled percentage. With 200 columns there is no way to find one.
- **Folded statistics are a bare list.** Category counts have no bars;
  "Distinct" and "Missing" sit among the values.
- **Footer**: "1 rows × 1 columns selected".
- **Grid**: text columns start too narrow ("GENE…"), and the picked
  column's grey band is heavy.

## Layout

```
┌ [▤] (py) df ●                         [Search…] [Filter] [Export ▾] ↻ ⧉ ⚙ │ action row (29px)
├ log2fc > 1 ×   cluster is NK ×   ↓ pvalue ×                    Clear all │ only with filters/sorting
├──────────────────────────┬─────────────────────────────────────────────┤
│ COLUMNS        6 of 6  ⋯ │  gene    cluster  log2fc  pvalue  counts …  │
│ [Find column          ]  │                                             │
│ ›☑ str  gene       ▁▁ 0% │                                             │
│ ⌄☑ cat  cluster   ███ 0% │                                             │
│    ■ Mono  ████████ 547  │                 the table                   │
│    ■ B cell ███████ 508  │                                             │
│    4 distinct · 0 missing│                                             │
│ ›☑ num  log2fc   ▂▅█ 6%  │                                             │
├──────────────────────────┴─────────────────────────────────────────────┤
│ 1 row × 1 column selected                       2,000 rows · 6 columns │ footer
└────────────────────────────────────────────────────────────────────────┘
```

### Action row

- **Left, what the table is**: the column panel toggle, the language icon,
  the table's name in the text colour (the full expression on hover), and
  a status dot after it, like the kernel picker's: green when live, amber
  while its session runs code, red on an error (the reason on hover).
- **Right, what you do with it**: Search (magnifier), **Filter** (adds a
  filter; shows "Filter · 2" while two are on), **Export ▾** (one menu:
  Copy as code, Copy filters as code (dimmed without filters), Save as
  CSV…), then the icons Read again, Open in a new window and Settings.
  Their order stays fixed, so nothing moves when filters change.

### Filter row

Shown only while filters or sorting are on, under the action row and over
the table (it is part of the table, like the column headers, not a second
toolbar): one chip per filter (click to change it, × removes it), one per
sort key ("↓ pvalue", click flips the direction, × removes it), and
"Clear all" at the right. Chips wrap to a second line rather than being
cut short. Sorting is otherwise visible only as a header arrow today.

### Column panel

- **Heading row**: "COLUMNS" (the small heading style used elsewhere),
  "6 of 6" shown, and a ⋯ menu with Show All and Hide All.
- **Find column**: a filter field under the heading; typing narrows the
  list by name.
- **One line per column** (32px instead of 42px): fold arrow, a small type
  badge (num, int, str, cat, bool, date, other; the dtype on hover), the
  name (cut with … only when needed), the sparkline, the missing share,
  and last a real checkbox (Kit's) to show or hide the column.
  The type line under the name goes; the missing count stays on hover.
  Hidden columns are faded. Clicking the name still scrolls the table to
  the column.
- **Folded statistics**, lined up with the name:
  - numbers: a wider histogram (the panel's width, 40px tall) with min and
    max under its ends, then Mean and Median;
  - categories: each common value with its colour square, a bar of its
    share and its count; clicking still filters to it;
  - a muted last line: "4 distinct · 120 missing (NaN)".

### Footer

Unchanged in place: the selection on the left with correct plurals
("1 row × 1 column selected"), the table's size on the right ("120 of
2,000 rows · 6 columns" when filtered).

### Table

- Text columns start as wide as their longest value in the first rows read
  (at most 320px); widths you dragged still win and are still saved.
- The picked column's band uses the lighter selection colour.

## Not changed

Reading in blocks, filtering and sorting in the kernel, the column ⋮ menu,
pinning, resizing, copying, the saved view per table, Open in a new window,
and what the daemon sends. This is an app-only change.

## Rules this changes in CLAUDE.md

The data explorer paragraph's toolbar sentence ("Copy as code …, Copy
filters as code …, a magnifier in Search, Open in a new window and an
icon-only gear") becomes the action row, filter row and Export menu above.
