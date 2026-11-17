# Comparing Sandhill with Positron

Status: **approved by Huan, 2026-11-16**. For the Sandhill website and the
GitHub page. Nothing is published until Huan has seen and approved it.

## What Huan asked for
- A comparison that shows where Sandhill is better than Positron, and
  where it isn't (2026-11-16).
- Performance matters.
- Movies, not animated images, and only for two things: smooth scrolling,
  and dragging a pane's edge, with a huge document open. If there is no
  difference, there are no movies.
- Huan's own desktop and private information must never show.
- Huan's Ubuntu server may be used for the server tests. Later dropped:
  no server test (Huan, 2026-11-17).
- Both apps in the light theme only (Huan, 2026-11-16).
- Fix the Sandhill bugs the tests found before publishing, then measure
  Sandhill again (Huan, 2026-11-17).
- No movies on the page after all: Huan saw no obvious difference in them
  (2026-11-17). The README gets the results, to show why to choose
  Sandhill; the full page is `docs/comparison/positron.md`.

## Keeping it fair
- Same Mac, same files, same tasks, same window, one app at a time.
  The Mac: Apple M1 Ultra, 64 GB, macOS 27.2.1, two 60 Hz displays. The
  window is 1878 x 1040 on the main display: both apps size new windows to
  the second, smaller display here, and that is the frame Positron settles
  on, so Sandhill is moved to match.
- Versions are written down: Positron 2026.11.1 (build 4), and Sandhill
  built from f916b05.
- Positron runs as a fresh install: empty `--user-data-dir` and
  `--extensions-dir`. On its first start it installs 15 extensions (Jupyter,
  Quarto, Ruff, Pyrefly, Air, Posit Assistant and others); those stay, as a
  new user gets them. Huan's own Positron and its extensions are not
  touched; it is quit during the measurements.
- Positron uses Huan's real home folder: with an empty one it stops at
  start-up waiting for the Mac's keychain (Huan chose this, 2026-11-16).
  R's and Python's start-up files are switched off for it.
- Sandhill runs from a test build with an empty home and state folder, never
  `/Applications/Sandhill.app` or its daemon.
- Every run starts from the same saved state of each app (no open files, no
  running sessions), with the other app not running.
- Scrolling and dragging are sent by a script, identical for both apps.
- Each measurement is taken five times, after one warm-up run that isn't
  kept; the page gives the middle value.
- Memory is what Activity Monitor shows (`footprint`), added up over all of
  an app's processes except R and Python sessions themselves. Positron
  starts R and Python when it opens, and Python again when a notebook
  opens; Sandhill starts them only when code runs.
- Measuring and filming are separate runs.
- The page says where Positron is better (Windows and Linux, the extension
  marketplace, the debugger, maturity, Posit behind it) and that Sandhill is
  not affiliated with Posit. No Positron logo.

## The documents
Built from public data only, the same files for both apps:
- A notebook of about 2,000 cells with stored outputs: text, tables and
  plots.
- A table of about 30,000 columns (genes), opened in each app's data
  viewer with `View()`.
- An R script of tens of thousands of lines.

## Performance
| Measure | How |
|---|---|
| Start-up | From launch to a usable window |
| Opening each document | From opening it to being able to scroll |
| Memory | All of an app's processes added together, at rest and with the documents open |
| Processor | Average while the scripted scrolling and dragging run |

## Smoothness, and the rule for movies
- The script scrolls each document at a steady speed, and drags the edge
  between two panes back and forth, in each app.
- The app's window is recorded at the display's 60 frames a second; a
  frame identical to the one before is a frame the app did not draw. That
  gives each app's frame rate and its longest freeze.
- Movies are made only if one app is clearly worse: at least 10 frames a
  second fewer, or freezes longer than 0.1 s, which people can see.
  Otherwise the page says both are smooth, with the numbers.
- Movies are MP4, side by side or one after the other, under 30 s.

## The server
- The test: start a ten-minute analysis on the server, quit the app
  halfway, reopen it, and see whether the analysis kept running and its
  output is there.
- It runs as a separate Linux user on the server, made for the test, so
  Huan's own Sandhill daemon, sessions and files there are not touched and
  his user name doesn't show. The server appears as `demo-server`.

## Privacy
- Only the app's window is recorded, with notifications off.
- Empty homes: no real paths, recent projects, accounts or history.
- The demo project's Git author is "Demo User".
- Agent account menus stay closed; any frame showing an email is filmed
  again, not blurred.
- File details (dates, software) are stripped from images and movies.
- Every image, and every movie frame by frame, is checked before Huan sees
  it.

## Where things go
| What | Where | Public? |
|---|---|---|
| Recordings, test runs, measuring scripts | `~/Documents/projects/sandhill-comparison-work/`, outside the repository | No |
| Demo project, data and the apps' empty homes | `/Users/Shared/sandhill-demo/`, so no path on screen contains Huan's user name | No |
| The page and approved screenshots | `docs/comparison/positron.md`, `docs/comparison/positron/` | After Huan approves |
| Approved movies | Uploaded through GitHub's web editor (kept outside the repository's history), later the website | After Huan approves |
