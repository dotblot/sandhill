# libghostty-vt's notices

The terminal links Ghostty's libghostty-vt, which `ghostty-vt-sys` builds
from source (Ghostty `83fb95624b04`, pinned in that crate's `GHOSTTY.lock`).
The Rust crates' own licences are in the generated notices; these are the
C, C++ and Zig libraries compiled into the app with it, taken unchanged from
the sources the build fetched (2026-11-14):

| Files | Library | Licence | Source |
|---|---|---|---|
| `ghostty-LICENSE` | Ghostty | MIT | `LICENSE-GHOSTTY` in the `ghostty-vt-sys` crate |
| `simdutf-LICENSE-MIT`, `simdutf-LICENSE-APACHE` | simdutf 9.0.0 | MIT or Apache-2.0 | github.com/simdutf/simdutf, tag `v9.0.0` (Ghostty vendors its amalgamated source) |
| `highway-LICENSE`, `highway-LICENSE-BSD3` | Highway `66486a10623f` | Apache-2.0 or BSD-3-Clause | deps.files.ghostty.org |
| `wuffs-LICENSE` | Wuffs `7411f488fe2e` | MIT and Apache-2.0, both texts in one file | deps.files.ghostty.org |
| `uucode-LICENSE.md`, `uucode-LICENSE_unicode`, `uucode-LICENSE_Bjoern_Hoehrmann` | uucode 0.2.0 (`ea62149739`) | MIT, with Unicode data and a UTF-8 decoder under their own terms | github.com/jacobsandlund/uucode at that commit |

They were found by what the release binary contains (simdutf, Highway and
Wuffs symbols) and by uucode being a required dependency. Ghostty's other
optional packages (zlib, fonts, GTK…) are not built for libghostty-vt.

When `ghostty-vt-sys` moves to another Ghostty, look at its `build.zig.zon`
and `pkg/` again and update these files.
