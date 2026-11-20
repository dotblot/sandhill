# Licences

Settled decisions about Sandhill's licence and code taken from elsewhere.
Moved here unchanged from `CLAUDE.md` on 2026-11-15. Brand marks and fonts
are in `look.md`.

- Positron and Kallichore are Elastic License 2.0: do not copy their code.
  Ark and Air are MIT and may be used.
- Open source and non-commercial (Huan, 2026-11-06): Sandhill is under
  GPL-3.0-or-later (`LICENSE`, and `license` in Sandhill's own `Cargo.toml`
  files), with no paid version. Code taken from elsewhere must be under a
  GPL-3.0-compatible licence; vendored dependencies retain their own licences.
  Sandhill.app carries every component's notices in `Resources/licenses`
  and beside each bundled program (2026-11-14): `notices/make.sh`
  (cargo-about) lists the Rust libraries of the app and the daemon, and a
  library under a licence not accepted in `notices/about.toml` fails the
  build; `notices/libghostty-vt/` holds the terminal engine's C/C++/Zig
  libraries' licences; Ark, ruff and ty keep theirs in `daemons/*.notices`.
- ComBat-ref (Huan, 2026-11-19): the Explore a Table R templates include
  Xiaoyu Zhang's MIT-licensed ComBat-ref code. Keep its copyright and
  permission notice with the code, including in the copy used by `steps.R`.
  `crates/daemon/assets/quick/combat_ref.R` records the pinned upstream
  revision and carries the full notice.
