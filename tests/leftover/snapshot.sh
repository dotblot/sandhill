#!/bin/bash
# The leftover test (docs/design/clean-footprint-plan.md, section 8): run in
# a fresh Mac user account, never in a real one. Needs no administrator
# rights. Writes to /Users/Shared/sandhill-leftover/<account>/.
#
#   snapshot.sh before   on the fresh account, before Sandhill is opened
#   snapshot.sh used     after using everything, right before Uninstall
#   snapshot.sh after    after Uninstall, once Sandhill has quit; writes
#                        report.txt comparing the three
set -u
stage=${1:?"before, used or after"}
# "~/" in the output is text for the report, not a path to expand.
# shellcheck disable=SC2088
case "$stage" in before|used|after) ;; *) echo "before, used or after"; exit 2;; esac

projects="$HOME/SandhillTest"
out="${SANDHILL_LEFTOVER_OUT:-/Users/Shared/sandhill-leftover/$(id -un)}"
mkdir -p "$out"
cd "$HOME" || exit 1

# Folders macOS keeps away from Terminal (it would ask) and the test's own
# projects, compared separately.
prune=(
  "./SandhillTest" "./.Trash" "./Desktop" "./Documents" "./Downloads"
  "./Movies" "./Music" "./Pictures" "./Library/Containers"
  "./Library/Group Containers" "./Library/Mail" "./Library/Messages"
  "./Library/Safari" "./Library/Metadata" "./Library/Biome"
  "./Library/Daemon Containers" "./Library/Application Support/AddressBook"
  "./Library/Application Support/CallHistoryDB" "./Library/Suggestions"
  "./Library/Application Support/Knowledge" "./Library/Application Support/FileProvider"
  "./Library/Application Support/com.apple.TCC" "./Library/Accounts"
  "./Library/Calendars" "./Library/Reminders" "./Library/Sharing"
  "./Library/Autosave Information" "./Library/IdentityServices"
  "./Library/Cookies" "./Library/HomeKit" "./Library/Trial"
)
expr=()
for p in "${prune[@]}"; do expr+=(-path "$p" -o); done
unset 'expr[${#expr[@]}-1]'

{
  echo "## files"
  find . -xdev \( "${expr[@]}" \) -prune -o -print 2>/dev/null | sed 's|^\.|~|' | LC_ALL=C sort
  echo "## shell files"
  for f in .zshenv .zprofile .zshrc .zlogin .bash_profile .bashrc .profile .config/fish/config.fish .condarc .Rprofile .Renviron; do
    [ -e "$f" ] && echo "~/$f $(shasum -a 256 < "$f" | cut -d' ' -f1)"
  done
  echo "## desktop"
  [ -e Desktop/sandhill-uninstall-leftovers.txt ] && echo "~/Desktop/sandhill-uninstall-leftovers.txt"
  echo "## temp"
  for d in /tmp "${TMPDIR:-/tmp}"; do
    find "$d" -maxdepth 1 -user "$(id -u)" -iname '*sandhill*' 2>/dev/null
  done | LC_ALL=C sort -u
  echo "## launchd"
  launchctl list 2>/dev/null | awk 'NR>1 {print $3}' | LC_ALL=C sort
  echo "## defaults"
  defaults domains 2>/dev/null | tr ',' '\n' | sed 's/^ *//' | LC_ALL=C sort
  echo "## keychain"
  security dump-keychain 2>/dev/null | sed -n 's/^ *"svce"<blob>="\(.*\)"$/\1/p' | LC_ALL=C sort -u
} > "$out/$stage.txt"

if [ -d "$projects" ]; then
  (cd "$projects" && find . -type f -print0 | LC_ALL=C sort -z | xargs -0 shasum -a 256) > "$out/$stage-projects.txt" 2>/dev/null
fi

echo "Saved $out/$stage.txt"
[ "$stage" = after ] || exit 0

section() { sed -n "/^## $2\$/,/^## /p" "$1" | sed '1d;/^## /d'; }
added() { LC_ALL=C comm -13 <(section "$out/before.txt" "$1" | LC_ALL=C sort) <(section "$out/after.txt" "$1" | LC_ALL=C sort); }

# Places R, Python, conda and the agents use: anything new there after
# uninstalling counts as a leftover, whatever its name.
known='sandhill|dev\.sandhill|^~/\.conda|^~/\.mamba|^~/\.cache/(conda|pip|uv|R)|^~/\.local|^~/Library/(R|Jupyter|Python)(/|$)|^~/Library/Caches/(pip|org\.R-project\.R)|^~/\.ipython|^~/\.jupyter|^~/miniforge3|^~/micromamba|^~/\.matplotlib|^~/\.config/matplotlib|^~/\.npm|^~/\.cargo|^~/\.venvs'
# What the user keeps by design: the agents' own sign-ins and settings.
kept='^~/\.claude|^~/\.codex'

{
  echo "Sandhill leftover test: $(id -un), $(date '+%Y-%m-%d %H:%M')"
  echo
  new=$(added files)
  leftovers=$(printf '%s\n' "$new" | grep -Ei "$known" | grep -Ev "$kept")
  kept_new=$(printf '%s\n' "$new" | grep -E "$kept")
  other=$(printf '%s\n' "$new" | grep -Eiv "$known" | grep -Ev "$kept")
  fail=0
  if [ -n "$leftovers" ]; then fail=1; echo "LEFT BEHIND (files):"; printf '%s\n' "$leftovers" | sed 's/^/  /'; echo; fi
  for s in "shell files" desktop temp launchd defaults keychain; do
    case "$s" in
      # Any change to a shell or R/conda settings file, the Desktop
      # leftovers list, or a temp folder of Sandhill's counts.
      "shell files"|desktop|temp) a=$(added "$s") ;;
      # macOS adds its own services, domains and keychain items too.
      *) a=$(added "$s" | grep -Ei 'sandhill') ;;
    esac
    if [ -n "$a" ]; then fail=1; echo "LEFT BEHIND ($s):"; printf '%s\n' "$a" | sed 's/^/  /'; echo; fi
  done
  if [ -f "$out/used-projects.txt" ] && [ -f "$out/after-projects.txt" ]; then
    if cmp -s "$out/used-projects.txt" "$out/after-projects.txt"; then
      echo "Projects: unchanged by Uninstall ($(wc -l < "$out/after-projects.txt" | tr -d ' ') files)."
    else
      fail=1
      echo "PROJECTS CHANGED BY UNINSTALL:"
      diff "$out/used-projects.txt" "$out/after-projects.txt" | sed 's/^/  /'
    fi
    echo
  fi
  [ -n "$kept_new" ] && { echo "Kept on purpose (the agents' own sign-ins):"; printf '%s\n' "$kept_new" | sed 's/^/  /'; echo; }
  if [ -n "$other" ]; then
    echo "Other new files (usually macOS's own; check for anything Sandhill made):"
    printf '%s\n' "$other" | sed 's/^/  /'
    echo
  fi
  [ "$fail" = 0 ] && echo "RESULT: PASS" || echo "RESULT: FAIL"
} > "$out/report.txt"
echo "Report: $out/report.txt"
tail -1 "$out/report.txt"
