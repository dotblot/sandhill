# Getting Ready: a Python environment

Huan, 2026-11-19. Status: approved by Huan (2026-11-19) as written, with
both package sets ticked by default and `sandhill-python` as the default
name.

Huan asked (2026-11-19) for Welcome to prepare Python too, when none is
ready, installing it with mamba. Explore a Table itself stays R only; the
Python kernel is for notebooks and scripts (such as the Scanpy notebook in
`sandhill-sample-real`).

## What Getting Ready shows

A third box, **Python**, between "R and its packages" and "AI helpers":

| Row | Shown |
|---|---|
| Python kernels | Each Python kernel Sandhill already finds, named as everywhere else ("Python 3.12 (omicverse)"). None: "No Python kernel yet". |
| mamba | The mamba (else conda) Sandhill would use and its folder, e.g. `~/miniforge3/bin/mamba 2.10.0`. None: the Install Miniforge row below. |
| Environments without a kernel | Only conda environments that have Python but not ipykernel, each with **Add ipykernel…**. |
| New environment | **Create a Python Environment…** |

The page's opening line becomes "What this Mac needs: R and its packages
for exploring tables, Python for notebooks, and the AI helpers."

## What it can do

Each action first shows the exact command in its row (Cancel, then the
action button), then runs as a job with progress and Show Details, as the
R installs do. Packages come from conda-forge only
(`-c conda-forge --override-channels`), never Anaconda's default channel.

1. **Create a Python Environment…** asks for:
   - a name (letters, digits, `-` and `_`; default `sandhill-python`;
     refused if an environment by that name exists),
   - the Python version (3.12 by default; 3.11 and 3.13 offered),
   - which packages (ticked boxes):
     - always: `python`, `ipykernel`;
     - Data analysis: `numpy`, `pandas`, `matplotlib`, `seaborn`,
       `scikit-learn`;
     - Single-cell: `scanpy`, `anndata`, `python-igraph`, `leidenalg`.

   It runs `mamba create -y -n <name> -c conda-forge --override-channels
   python=<version> ipykernel ...`. The new environment's kernel then
   appears in every kernel picker by itself: Sandhill already finds conda
   environments with ipykernel.
2. **Add ipykernel…** for an existing environment runs `mamba install -y
   -n <env> -c conda-forge --override-channels ipykernel`. Nothing else in
   the environment is updated (`--freeze-installed`).
3. **Install Miniforge…**, only when no mamba or conda is found: downloads
   conda-forge's official installer
   (`https://github.com/conda-forge/miniforge/releases/latest/download/Miniforge3-$(uname)-$(uname -m).sh`)
   and runs it with `-b -p ~/miniforge3`. Batch mode leaves the shell's
   startup files alone; the row says how to add `mamba` to the Terminal
   yourself (`~/miniforge3/bin/mamba shell init`). Sandhill finds
   `~/miniforge3` without that.

## Undo

The record of what Sandhill installed (`installed-by-sandhill.tsv`) gains
three kinds, and **Remove What Sandhill Installed…** handles them:
- an environment Sandhill created: `mamba env remove -y -n <name>`, if it
  still exists at the recorded place;
- ipykernel added to an environment: `mamba remove -y -n <env> ipykernel`,
  only if it is still the recorded version;
- Miniforge: removed (`rm -rf ~/miniforge3`) only when it holds no
  environment other than ones Sandhill created and nothing has been added
  to its base since; otherwise it stays, and the row says so.

## Settled rules this changes
- `docs/decisions/devices.md`: "never user Python/R, conda environments"
  gets the same exception the agents got: what Sandhill's record says it
  created (environments, ipykernel, Miniforge) can be removed, nothing else.
- `docs/decisions/editor-and-kernels.md` says kernel pickers list no
  environments without a kernel. That rule stays for the pickers; Getting
  Ready lists such environments only to offer Add ipykernel.
- `docs/decisions/window.md`: Getting Ready gains the Python box.

## Checks
- Daemon tests: finding mamba/conda and environments without ipykernel
  (fake prefixes); the create, install and remove commands (names refused,
  conda-forge only, nothing run outside the recorded items); the record
  and its "still as Sandhill left it" checks.
- A real run on this Mac into a throwaway environment, then removal.
- On screen after Huan agrees to a test build: the Python box, each
  confirm row, and a new kernel appearing in a notebook's picker.
