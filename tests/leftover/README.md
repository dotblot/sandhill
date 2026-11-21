# The leftover test

Sandhill's promise (`docs/design/clean-footprint-plan.md`, section 8):
after Uninstall Sandhill, nothing is left on the computer except the
user's projects, data and results (and, by design, the agents' own
sign-ins). This test checks it by hand before a release, in a new Mac
user account made only for it. It never runs in a real account.

`snapshot.sh` lists what is on the account at three moments and writes a
report. It needs no administrator rights and writes only to
`/Users/Shared/sandhill-leftover/<account>/`.

## Before you start (in your own account)

1. A test build of Sandhill and the three snapshot files are in
   `/Users/Shared/sandhill-leftover/` (Main puts them there).
2. **System Settings → Users & Groups → Add User…**: a **Standard**
   account named `Sandhill Test` (account name `sandhilltest`), any
   password. This needs your administrator password.
3. **Apple menu → log in as Sandhill Test** (your own account stays
   logged in). Skip every setup question.

## In the Sandhill Test account

4. In Finder, **Go → Go to Folder…** `/Users/Shared/sandhill-leftover`.
   Double-click **1 Before.command**. It makes `~/SandhillTest` (the test
   projects, with `example.csv`), `~/Library/R/arm64/4.6/library` (an R
   library of your own, as many R users have; Sandhill never makes one)
   and `~/Applications`, then takes the first snapshot. If macOS asks
   whether Terminal may open it, allow it.
5. Drag **Sandhill** from that folder into the account's own
   **Applications** folder (in Finder: **Go → Home**, then
   `Applications`). Not the main `/Applications` folder. Open it
   (Control-click → Open the first time).
6. Use everything, in this order:
   1. **First run: Use what's on this computer**, Bulk RNA-seq not
      ticked (Homebrew's R would build it for half an hour). Continue.
   2. **R and Python**: on R's card, **Add Packages**, search `praise`,
      Install into `~/Library/R/arm64/4.6/library`. On Python, there is
      no conda or uv: **Install Miniforge…**, then **Create a Python
      Environment…** with Data analysis only.
   3. **Manage → Agents**: install **Claude Code** (no need to sign in).
      Codex needs Homebrew's npm, which this account can't write to; its
      row should say so.
   4. **R and Python**: switch to **Everything in one folder**, R and
      Python ticked, core only, and tick **Also the packages in my
      current Python** (the Miniforge environment). **Set Up and Copy**.
      This downloads about 500 MB.
   5. Open the folder `~/SandhillTest/project1` as a project. Make a
      notebook with a Python cell `import numpy; print(numpy.__version__)`
      and run it. In the Packages pane install `requests`. In the
      Terminal pane, type `pip install rich`, then
      `touch ~/miniforge3/sandhill-lock-test`: it should be refused
      (the lock).
   6. **Project Settings → This project's packages → Save This
      Project's Packages…** for Python.
   7. **Explore a Table**: choose `~/SandhillTest/example.csv` and make
      one plot.
   8. **Manage → On This Computer**: look at the list.
7. Quit nothing. Double-click **2 Used.command** (the snapshot right
   before uninstalling).
8. **Manage → On This Computer → Uninstall Sandhill**: tick everything,
   including Claude Code and **Also delete Sandhill's history**.
   **Review…**, then **Uninstall**. Sandhill quits and moves itself to
   the Trash.
9. Wait one minute, then double-click **3 After.command**. It writes
   `report.txt` and shows **RESULT: PASS** or **RESULT: FAIL**.
10. Log out of Sandhill Test and tell Main it's done; Main reads the
    report from `/Users/Shared/sandhill-leftover/sandhilltest/`.

## Afterwards

11. In your own account: **System Settings → Users & Groups**, select
    Sandhill Test, **Delete User…**, **Delete the home folder**.

## Reading the report

- **LEFT BEHIND**: a failure. Files or settings Sandhill made that
  Uninstall didn't remove, changed shell or R settings files, a Desktop
  leftovers list, Sandhill's launchd jobs, preference domains or
  Keychain items.
- **Projects**: the test projects must be exactly as they were right
  before Uninstall.
- **Kept on purpose**: the agents' own sign-ins (`~/.claude`,
  `~/.codex`), which Uninstall never removes.
- **Other new files**: usually macOS's own (caches, logs); read them for
  anything that came from Sandhill.
