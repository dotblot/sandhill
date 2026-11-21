# Clean install, clean uninstall

Huan, 2026-11-20. Status: approved by Huan (2026-11-20), with his
decisions on the lock, the uninstall ticks and the kits, and GSVA kept
with Install Developer Tools. Replaces the draft "Sandhill's own R and
Python" plan of the same day.

## The promise

Huan wants this to be one of Sandhill's biggest selling points: nobody
should have to search how to remove Sandhill completely without harming
their computer. Installing is clean, uninstalling is clean, and nothing is
left behind except the user's data, results and projects. Nothing is
installed without the user seeing what, with which tool, and where.

## 1. The first question

The first time Sandhill runs, Welcome asks **"Where should Sandhill's R and
Python come from?"**:

- **Use what's on this computer.** Sandhill uses the R, Python and tools
  already installed, and asks before adding anything (section 2).
- **Everything in one folder.** Sandhill sets up its own R and Python in
  one folder, apart from the rest of the computer (section 3). The folder
  is `~/.sandhill` unless the user picks another (conda refuses paths
  with spaces, so such a folder is refused with that reason).

The answer can be changed later in Getting Ready (section 4). The AI
agents are not part of this choice (section 5).

## 2. Use what's on this computer

**Checking.** Getting Ready finds what Sandhill needs (R, an R kernel,
the Explore packages, Python, ipykernel) and the install tools present:
R's own installer (`install.packages()`, `BiocManager::install()`), pip,
uv and mamba/conda. It shows each with its version and folder.

**Asking.** When something is missing, nothing installs until the user has
seen, in its row:
- **what** will be installed, with versions where known;
- **which tool** installs it, from those present that suit the target:

  | Target | Tools offered (first one suggested) |
  |---|---|
  | A CRAN or Homebrew R | R's own installer |
  | A conda R | mamba/conda (conda-forge, bioconda), R's own installer |
  | A conda environment's Python | mamba/conda, pip |
  | A uv or venv environment | uv, pip |
  | A Python the system manages (it refuses pip) | none: Sandhill offers to make an environment first |

- **which folder** it goes into: one of R's libraries (`.libPaths()`), or
  the chosen environment's folder, with its full path;
- the **exact command**.

Then Cancel, or **Install**. When nothing at all is there (no R, or no
Python), the row offers the official ways with the same details: the R
installer from CRAN, Homebrew, or a mamba environment; for Python, a uv or
mamba environment. Sandhill never creates R libraries or edits shell
startup files in this mode.

**Install without asking.** A switch in Getting Ready, off by default.
When on, Sandhill installs what is missing with the tool and folder last
chosen for that language, without the question, and shows a notice
afterwards naming what, where and how. It never applies to a target the
user hasn't chosen before.

**The record.** Every install Sandhill makes in this mode, asked or not,
goes into the record (`installed-by-sandhill.tsv`, already used by Getting
Ready): what, version, tool, folder, when, and why (for example "Find
Changed Genes needed DESeq2"). This includes IRkernel's kernel file and
ipykernel added to an environment.

**Removing.** **What Sandhill Installed** lists the record with ticks.
The user removes some or all, each with the tool that installed it (for
example `remove.packages()`, `pip uninstall`, `uv pip uninstall`, `mamba
remove`). Only items still at their recorded version and place are
removed; anything updated since is the user's and stays, as today.

## 3. Everything in one folder

One job sets up Sandhill's own R and Python in the folder, after showing
what, the download size and the folder (Cancel, then **Set Up**):

- **micromamba**, conda's one-file installer, kept in the folder and never
  added to the shell's PATH or startup files;
- **`sandhill-r`**: R, IRkernel and the Explore packages, from
  conda-forge and bioconda;
- **`sandhill-python`**: Python, ipykernel and the data-analysis and
  single-cell packages, from conda-forge.

Measured on this Mac by a dry run (2026-11-20): about 710 MB to download,
a few GB unpacked. Every package comes ready-built, so no administrator
password and no Apple developer tools are needed. Bioconda's Bioconductor
packages are not yet built for R 4.6, so this R is **R 4.5.3** (DESeq2
1.50.2), one release behind; it moves up when bioconda does.

**Everything inside Sandhill uses only the folder:**

| Where | What Sandhill does |
|---|---|
| Kernel pickers, Console, notebooks, scripts, Explore | Only Sandhill's own R and Python are listed and used. A document asking for another kernel gets the "wasn't found" message, saying Getting Ready can switch. |
| R | Its only library is `sandhill-r`'s: `install.packages()`, `BiocManager::install()`, `remotes::install_github()` and `remove.packages()` all act there. The home folder's `~/.Renviron` and `~/.Rprofile` are not read; a project's own `.Rprofile` (renv) still runs. R's cache, data and config folders are inside. |
| Python | `pip install`, `%pip` and even `pip install --user` stay inside; `PYTHONPATH` is cleared; pip's, IPython's, Jupyter's and matplotlib's settings and caches are inside. |
| Terminal | The user's shell with the same settings, and Sandhill's environments first on PATH, so `R`, `Rscript`, `python`, `pip` and `micromamba` typed there are Sandhill's. |
| Commands the agents run | The same settings as the Terminal. |
| Getting Ready | **Install a Package…** takes a name or a GitHub `owner/repo` and installs into the folder only: conda first, then CRAN, Bioconductor or PyPI, then GitHub. |
| A lock | The commands Sandhill starts (kernels, Terminal, agents' commands) are not allowed to write into the R libraries, Pythons, conda folders and Homebrew folder Sandhill knows are the user's (Apple's sandbox on a Mac; bubblewrap on Linux when it works there). Huan, 2026-11-20: build it. |

The lock covers the folders Sandhill has found, not every folder on the
computer; the user's own files stay writable, because code in an IDE
saves results into projects. Reading them still works. A refused write
says in plain words that Sandhill's own environment is chosen and how to
switch.

## 4. Switching later

From "Use what's on this computer" to "Everything in one folder",
Getting Ready asks which packages the new environment gets:

- **Only what Sandhill needs** (the core packages above), or
- **Also the packages in my current R / Python**: the user picks the R
  (its libraries) or Python environment to copy from.

For a copy, Sandhill first shows a table of every package and where it
will come from: conda with the same version, conda with another version
(both shown), CRAN or Bioconductor, PyPI, GitHub (read from the package's
own record of where it was installed from), or **can't be copied** with
the reason (for example a package built from a local folder). Then
**Copy**. The user's current R and Python are only read, never changed.
Afterwards a report lists anything that failed.

The other way, back to "Use what's on this computer", is a switch; the
folder stays until the user removes it.

## 5. The AI agents

Claude Code and Codex are not part of the computing environment. They are
installed for the whole user account, only in their official ways, the
command shown before it runs:
- Claude Code: `curl -fsSL https://claude.ai/install.sh | bash`;
- Codex: `npm install -g @openai/codex`, or `brew install --cask codex`
  when Homebrew is there.

Each install Sandhill makes is recorded. Sandhill never writes into an
agent's own settings to connect to it; where an agent can only be
connected through its settings file (Antigravity's
`~/.gemini/config/mcp_config.json` today), Sandhill asks first, records
the entry, and removes exactly that entry when uninstalled.

## 6. Sandhill's own files: one folder

Sandhill's own files move into `~/.sandhill` (or the folder chosen in
section 1), on this computer and on remote servers alike: the daemon and
its helper programs (ark, ruff, ty), state, history, settings, accounts,
caches and, in that mode, the computing environment. They are spread over
`~/.local/bin`, `~/.local/share/sandhill`, `~/.local/state/sandhill`,
`~/Library/Application Support/Sandhill` and `~/Library/Caches` today, and
move on first start of the new version.

The places the operating system forces stay few, fixed and listed:
- the daemon's start at login: on a Mac registered from inside the app
  (`SMAppService`), so no file in `~/Library/LaunchAgents`; on Linux the
  user service file in `~/.config/systemd/user`;
- the folders macOS makes for every app, named by Sandhill's app id
  (WebKit data, HTTP storage, caches, saved window state, preferences).

## 7. Seeing and removing it all

- **Manage → What Sandhill Has Put on This Computer**: every folder and
  file outside the user's projects, with sizes, the record of installs into
  the user's environments, and the agents Sandhill installed. Visible at
  any time, not only when leaving.
- **Manage → Uninstall Sandhill**, asked in its card:
  - always removed: the Sandhill folder (with its own R and Python), the
    forced places of section 6, the agent settings entries Sandhill added;
  - listed with ticks, all ticked: what Sandhill installed into the
    user's own R and Python (section 2) (Huan, 2026-11-20);
  - listed with ticks, none ticked: the agents Sandhill installed, removed
    in their official way if ticked; their sign-ins stay (Huan,
    2026-11-20);
  - one question: also delete Sandhill's history (conversations, file
    history)?
  - never touched: projects, their `.sandhill` folders, data and results.
  - It ends with a report: what was removed, and anything that couldn't
    be, with its path. Then the app moves itself to the Trash, as today.

## 8. Proving it

A **leftover test** for each release: on a fresh Mac user account and a
fresh Linux machine, list every file outside the test projects; install
Sandhill; use every part (both modes, Getting Ready, installs and removals,
Install Without Asking, switching with a copy, agents, Terminal, Explore,
notebooks); uninstall; list again. Any difference other than the projects
fails the test. It runs by hand before a release, not on every push.

## 9. Everything inside Sandhill, no Terminal

Every part of preparing and managing R and Python is a button or a list
in Sandhill. The Terminal is never needed; it stays available for those
who like it.

**First run: three short steps** on Welcome, each with a suggested answer
already picked, so a beginner can press Continue three times:
1. **Where R and Python come from** (section 1), with what Sandhill found:
   "We found R 4.6.2 (Homebrew) and Python 3.12 (conda, omicverse) on this
   computer."
2. **What you will work on**, as ticks: the kits below. Beginners choose
   tasks, not packages; each kit shows its packages under it.
3. **Review**: what will be installed, where, the download size, the free
   space, about how long; then **Set Up**. A progress card follows, and
   Sandhill can be used meanwhile.

"Set up later" skips it; Getting Ready can do it any time.

**The kits** (Huan, 2026-11-20). A small core always comes, whichever
kits are ticked:

| Kit | Packages |
|---|---|
| Core (always) | R: IRkernel, jsonlite, ggplot2, ggrepel, pheatmap, readxl, writexl, svglite. Python: ipykernel, numpy, pandas, matplotlib |
| Bulk RNA-seq | DESeq2, edgeR, limma, apeglm, sva, RUVSeq, tximport, AnnotationDbi, org.Hs.eg.db, org.Mm.eg.db |
| Single-cell in R | Seurat, SeuratObject, harmony, glmGamPoi, SingleCellExperiment, scater, scran, scDblFinder |
| Single-cell in Python | scanpy, anndata, python-igraph, leidenalg, harmonypy, seaborn, scikit-learn |
| Plots and reports | ComplexHeatmap, patchwork, cowplot, ggpubr, rmarkdown, knitr, Quarto; seaborn |
| Pathways and gene sets | clusterProfiler, enrichplot, msigdbr, fgsea, GSVA, ReactomePA; gseapy, decoupler |

scvi-tools stays out (it brings PyTorch, about 1 GB); Add Packages
offers it. Spatial, ATAC/ChIP and variant kits may come later.

Measured by dry runs on this Mac for "Everything in one folder"
(2026-11-20, conda-forge and bioconda, nothing installed):

| Environment | Packages | Download |
|---|---|---|
| R, core only | 158 | 274 MB |
| R, core and Bulk RNA-seq | 280 | 429 MB |
| R, core and Single-cell in R | 344 | 560 MB |
| R, core and Plots and reports | 294 | 536 MB |
| R, every kit (without GSVA) | 520 | 837 MB |
| Python, core only | 119 | 110 MB |
| Python, core and Single-cell in Python | 201 | 211 MB |
| Python, every kit | 261 | 251 MB |

Everything at once is about 1.1 GB to download. Python's gseapy comes
from bioconda, so Python uses bioconda too.

**GSVA on Apple-silicon Macs:** bioconda has no build of GSVA for them
(it has one for Intel Macs and Linux). In "Everything in one folder" on
such a Mac, Sandhill builds GSVA from Bioconductor's source, which needs
Apple's developer tools; the Review step says so and offers **Install
Developer Tools** (Apple's own installer window). Without them, the
Pathways kit installs without GSVA and says which package is missing
and why.

**One home for R and Python: Getting Ready becomes "R and Python"**, on
Welcome and in Manage. One card per environment (Sandhill's own, or each
one found), each with:
- a plain status line: "Ready for Explore a Table and bulk RNA-seq",
  "2 packages missing for single-cell", "Needs repair: 3 packages were
  built for R 4.5";
- its size and folder;
- **Add Packages**: one search over CRAN, Bioconductor, conda-forge,
  bioconda and PyPI at once, each result with its one-line description and
  where it comes from; a GitHub `owner/repo` or a downloaded package file
  works too; kits can be added later the same way;
- **Updates**: what has a newer version, with Update or Update All;
- **Check and Repair**: loads every package and starts the kernel; whatever
  fails gets a plain reason and a Repair button;
- **Undo Last Change**: puts back the packages as they were before the
  last install, update or removal (conda's own history for conda
  environments; for R libraries, Sandhill keeps the replaced package
  folders in its own folder for 30 days);
- **More**: New Environment, Duplicate, Export, Import, Delete.

The Packages pane in the project window stays the per-session view. Its
Install, Update and Remove use the same confirmation, tool and folder
rules and the same record as Getting Ready; they no longer type commands
into the Console.

**Missing packages found for you.** When a script, notebook or Quarto file
opens, Sandhill reads its `library()`, `require()`, `pkg::` and `import`
lines and, if any package is missing from the document's kernel, shows one
bar: "This notebook uses Seurat and harmony, which aren't installed. Install
them?" When code stops with "there is no package called" or
`ModuleNotFoundError`, the error gets an **Install** button beside Fix and
Explain. Import names are matched to package names (`sklearn` is
`scikit-learn`, `cv2` is `opencv`).

**Errors in plain words, with a fix.** Install failures Sandhill
recognises say what happened and offer the fix as a button:
- a package that must be compiled, on a Mac without Apple's developer
  tools: **Install Developer Tools** opens Apple's own installer window;
- no internet or a blocked site: which site, and Choose Another Source;
- packages that can't be installed together: which ones, and why;
- not enough space: how much is needed, and Free Up Space.

The full log is always one click away (Show Details).

**Projects carry their environment.** **Save This Project's Packages**
writes the standard files beside the project (`renv.lock` for R;
`environment.yml` or `requirements.txt` for Python). A project that
arrives with such a file offers **Set Up This Project's Packages** in one
click. Collaborators and reviewers get the same versions, and the methods
text can list them.

## 10. For beginners and experts alike

One Sandhill for both, not a "beginner mode": people grow, and two modes
would double what is built and explained.

- **Plain by default.** Every row reads as a sentence in plain words:
  "Install DESeq2, needed for Find Changed Genes, into Sandhill's folder
  (25 MB)". Words like channel, library path and solver don't appear on
  the surface; where one helps, its tooltip explains it.
- **The suggested choice is always picked.** A beginner never has to
  choose a tool, folder or source; Continue does the suggested thing.
- **Details one click away, never hidden.** Each row's Details shows and
  lets an expert change the tool, the folder, the sources and the
  version, and shows the exact command with Copy. That satisfies "no
  hidden installation" without crowding the beginner.
- **Expert settings in one place** (R and Python → Advanced): the
  suggested tool for each language, the sources and their mirrors, the
  Bioconductor release, Install Without Asking, and **Open Terminal Here**
  for an environment (optional, never required).
- **No lock-in.** Sandhill's environments are ordinary conda environments,
  and its project files are the standard ones, so experts can use them
  outside Sandhill and leave Sandhill without converting anything.

## 11. Also handled

- **Download sources and mirrors.** Sandhill picks the fastest working
  source for CRAN, Bioconductor, conda-forge, bioconda and PyPI, including
  the mainland China mirrors (Tsinghua, USTC), and follows the system's
  proxy. Shown and changeable under Advanced.
- **Space first.** Before any install, Sandhill checks free space against
  the size and says so before starting, not halfway.
- **Interrupted installs.** Quitting, sleep or a lost connection during
  an install leaves no half-made environment: Sandhill resumes it or
  undoes it on the next start.
- **R upgrades.** When R moves to a new minor version (Homebrew's
  4.6 to 4.7, say), Sandhill notices packages built for the old one and
  offers to reinstall them for the new one, from the same sources.
- **Remote computers.** The same R and Python page manages environments
  on a server Sandhill connects to, into its own `~/.sandhill` there.
- **If the app is simply dragged to the Trash.** What remains is the one
  folder and the few forced places of section 6, listed on Sandhill's
  website and in its README with how to delete them.

## Settled rules this changes

- `docs/decisions/window.md`: Welcome's three first-run steps; Getting
  Ready becomes "R and Python" (section 9) with its modes, asking,
  Install Without Asking, switching and copying; Install Miniforge… and
  Create a Python Environment become part of section 2's asking;
  "Sandhill carries no R or packages" becomes "only when asked, into its
  folder"; bioconda joins conda-forge for R.
- `docs/decisions/editor-and-kernels.md`: the Packages pane installs
  through the same confirmation and record instead of typing into the
  Console; the missing-packages bar and the Install button on errors.
- `docs/decisions/devices.md`: one folder for Sandhill's files; the
  uninstall of section 7; the footprint page; the leftover test.
- `docs/decisions/editor-and-kernels.md`: in "Everything in one folder",
  pickers list only Sandhill's own kernels.
- `docs/decisions/agents.md`: agents installed only in their official
  ways; no writing into agents' settings without asking.

## Building order

Each step is usable on its own and committed when done:
1. Section 2: asking with tool and folder, Install Without Asking, the
   wider record, What Sandhill Installed with ticks; the Packages pane on
   the same rules. Built 2026-11-20.
2. Section 3 and the first-run steps of section 9: Sandhill's own
   environment and kits, with the Terminal, kernels and agents' commands
   kept inside. Built 2026-11-20.
3. Section 9's R and Python page: Add Packages search, Updates, Check and
   Repair, Undo Last Change, More. Built 2026-11-20.
4. Section 9's missing-packages bar, Install on errors, plain errors.
5. Section 4: switching and copying packages; section 9's project files.
6. Section 6: one folder for Sandhill's own files, the move, `SMAppService`.
7. Sections 5 and 7: agents' settings, the footprint page, the new
   uninstall.
8. Section 11, the lock and remote computers.
9. Section 8: the leftover test, run once on this Mac in a fresh user
   account after Huan agrees.

## Decided (Huan, 2026-11-20)

- At uninstall: installs into the user's own R and Python ticked by
  default, agents not ticked.
- The lock: build it.
- The kits: the five above, as listed.

## To confirm while building

- Confirmed 2026-11-20: micromamba 2.10 ignores `register_envs: false`
  and writes `~/.conda/environments.txt`, `~/.mamba/pkgs` and
  `~/.cache/conda`. Sandhill therefore runs it with `HOME`,
  `XDG_CACHE_HOME`, `CONDARC` and `MAMBA_ROOT_PREFIX` inside its folder
  (and `pkgs_dirs` in its condarc); a test run wrote nothing outside.
- That a project's `.Rprofile` can run from Sandhill's own R profile
  without running the home folder's.
- How each macOS per-app folder is named for Sandhill (from a real run).
