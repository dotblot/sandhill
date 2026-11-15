# The plot zoom and save window

Huan, 2026-11-14. Status: approved, with a title-row button that hides or
shows the settings column (remembered on this Mac).

## What is wrong

The preview sits on top, and a 190px footer below stacks five left-aligned
rows: formats, size, two switches, a status line, then folder, name and
Save. The right half of the footer is empty, and the footer takes height
from the plot even in a wide window.

## Layout

The preview keeps the whole height on the left. A 260px settings column
on the right holds everything else, top to bottom:

- **Format**: PNG · SVG · PDF, one segmented control.
- **Size**: Width and Height with the unit menu (px, in, cm); Resolution
  (dpi) under them; "Size follows window" switch.
- **PDF only: Rasterize** switch, "Draw the plot as pixels at the resolution
  above; smaller files for plots with many points." Off by default,
  remembered with the other sizing choices for the project on this Mac.
- **Preview**: "Redraw preview" switch (as now, with its reason on hover).
- **Save to**: the folder (path, Choose…) and the file name.
- **Save** (or Replace), full width, at the bottom of the column.

The status line ("Drag the title to move…", errors) moves under the
preview, where it explains what the preview shows. Title row, moving,
resizing, previous/next and closing are unchanged. Below 720px wide the
column moves under the preview as two columns of controls.

## Rasterizing a PDF

Only for plots Sandhill can draw again (the "Saved as drawn" ones keep
what they are):

- **Python (matplotlib)**: the data layers (points, lines, images, areas)
  become pixels at the chosen resolution; axes, ticks, labels and titles
  stay vector text, so the PDF stays sharp where it is read.
- **R**: R's PDF device cannot mix the two, so the whole plot is drawn as
  one picture at the chosen resolution inside a PDF page of the chosen
  size (text included), using only base R.

## Not changed

Formats, units, sizes, the folder chooser, the redraw rules and the file
names stay as they are.
