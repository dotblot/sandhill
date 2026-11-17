"""Draws docs/readme/performance.svg, the README's chart of the Positron
comparison. The numbers are the middle of five runs from
docs/comparison/positron.md (2026-11-17); change both together.

    python3 docs/readme/performance.py
"""

from pathlib import Path

import matplotlib

matplotlib.use("Agg")
import matplotlib.pyplot as plt  # noqa: E402

SANDHILL = "#142844"  # the crane's navy (docs/decisions/look.md)
POSITRON = "#a7adb5"  # neutral grey, not Positron's own colours
INK = "#1f2328"
MUTED = "#59636e"

TASKS = [
    "Open a 2,002-cell\nnotebook",
    "Open a 32,738-column\ntable",
    "Open a 30,005-line\nR script",
    "Start the app",
]
# (Sandhill, Positron) per task, in TASKS order.
SECONDS = [(0.64, 180), (1.05, 0.57), (0.76, 0.58), (2.05, 3.49)]
MEMORY_MB = [(195, 4503), (137, 1601), (155, 1024), (137, 955)]
SCROLL = ["Notebook", "R script"]
SCROLL_CORES = [(0.34, 1.12), (0.22, 0.40)]


def seconds(v):
    return f"{v:.0f} s" if v >= 10 else f"{v:.2f} s"


def megabytes(v):
    return f"{v / 1000:.1f} GB" if v >= 1000 else f"{v:.0f} MB"


def cores(v):
    return f"{v:.2f}"


def panel(ax, labels, pairs, fmt, title, ticks=None):
    """Sandhill's bar above Positron's, rows top to bottom in list order.
    `ticks` (value, label) makes the x axis logarithmic with those marks."""
    rows = range(len(labels))
    h = 0.36
    top = max(max(p) for p in pairs)
    for offset, colour, i in ((-h / 2, SANDHILL, 0), (h / 2, POSITRON, 1)):
        values = [p[i] for p in pairs]
        ys = [r + offset for r in rows]
        ax.barh(ys, values, height=h * 0.92, color=colour)
        for y, v in zip(ys, values):
            x = v * 1.12 if ticks else v + top * 0.02
            ax.text(x, y, fmt(v), va="center", ha="left", fontsize=9.5, color=INK)
    ax.set_yticks(list(rows), labels, fontsize=10, color=INK)
    ax.set_ylim(len(labels) - 0.5, -0.5)  # first row at the top
    ax.set_title(title, loc="left", fontsize=11, color=INK, pad=10)
    if ticks:
        ax.set_xscale("log")
        ax.set_xlim(ticks[0][0] / 2, top * 12)
        ax.set_xticks([v for v, _ in ticks], [t for _, t in ticks])
        ax.minorticks_off()
    else:
        ax.set_xlim(0, top * 1.3)
    ax.tick_params(axis="x", labelsize=8.5, colors=MUTED, length=0)
    ax.tick_params(axis="y", length=0)
    ax.grid(axis="x", color="#e6e8eb", linewidth=0.8)
    ax.set_axisbelow(True)
    for side in ("top", "right", "left"):
        ax.spines[side].set_visible(False)
    ax.spines["bottom"].set_color("#d0d7de")


def main():
    plt.rcParams["font.family"] = ["Helvetica", "Arial", "DejaVu Sans"]
    plt.rcParams["svg.fonttype"] = "path"  # the same shapes on every machine
    fig = plt.figure(figsize=(10, 5.0), facecolor="white")
    grid = fig.add_gridspec(
        2, 2, height_ratios=[4, 2], width_ratios=[1, 1],
        hspace=0.45, wspace=0.08, left=0.17, right=0.98, top=0.83, bottom=0.06,
    )
    time_ax = fig.add_subplot(grid[0, 0])
    memory_ax = fig.add_subplot(grid[0, 1])
    panel(time_ax, TASKS, SECONDS, seconds, "Time (log scale; shorter is better)",
          [(0.1, "0.1 s"), (1, "1 s"), (10, "10 s"), (100, "100 s")])
    panel(memory_ax, TASKS, MEMORY_MB, megabytes, "Memory (log scale; less is better)",
          [(100, "100 MB"), (1000, "1 GB"), (10000, "10 GB")])
    memory_ax.tick_params(axis="y", labelleft=False)
    scroll_ax = fig.add_subplot(grid[1, :])
    panel(
        scroll_ax, ["Scroll the\nnotebook", "Scroll the\nR script"], SCROLL_CORES, cores,
        "Processor cores used while scrolling (less is better; both at about 60 frames a second)",
    )
    fig.text(0.17, 0.95, "Sandhill and Positron on the same Mac", fontsize=13,
             fontweight="bold", color=INK)
    fig.text(0.17, 0.905,
             "Middle of five runs. Memory leaves out the R and Python sessions. "
             "Apple M1 Ultra, macOS 27.2.1, Positron 2026.11.1, November 2026.",
             fontsize=9, color=MUTED)
    handles = [
        plt.Rectangle((0, 0), 1, 1, color=SANDHILL),
        plt.Rectangle((0, 0), 1, 1, color=POSITRON),
    ]
    fig.legend(handles, ["Sandhill", "Positron"], loc="upper right",
               bbox_to_anchor=(0.98, 0.985), ncol=2, frameon=False, fontsize=10)
    out = Path(__file__).with_name("performance.svg")
    fig.savefig(out, format="svg", metadata={"Date": None, "Creator": None})
    print(out)


if __name__ == "__main__":
    main()
