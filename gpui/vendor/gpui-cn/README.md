# gpui-cn

shadcn-style components for GPUI, built on the `gpui-base` layer of
[gpui-kit](https://gpui-kit.com/).

`gpui-base` owns the behavior: focus, keyboard activation, accessibility,
tooltips, and motion. gpui-cn owns the look: a theme derived from a
surface color, an ink color, and an accent color, and components styled
from that theme.

## Sandhill vendored changes

- The terminal is retained from gpui-cn 0.8.0 and adapted to Kit 0.9 (0.9.1).
- Terminal file references support POSIX absolute and relative paths, optional
  one-based `:line:column` suffixes, and quoted Python traceback filenames.
  `FileLinkHovered` asks the embedding app for asynchronous validation;
  `confirm_file_link` enables the existing hover underline only after approval.
  Platform-click emits `FileLinkOpened`, while URL links retain the browser
  opener. Sandhill resolves against OSC 7 cwd (then terminal start cwd), limits
  links to the project, and validates files through the connected daemon with
  a short-lived hover cache.
- `TerminalState::frame_read` grants the next frame's credit for a terminal
  that is never painted (an agent's, while its chat is shown), so the host can
  still read its screen; painted terminals keep the paint-paced credit.

## Install

```bash
cargo add gpui-kit gpui-cn
```

This adds the two dependencies to `Cargo.toml`:

```toml
[dependencies]
gpui-kit = "0.7"
gpui-cn = "0.1"
```

gpui-cn runs on [GPUI Fast](https://crates.io/crates/gpui-fast) by default,
through its `gpui-fast` feature. To run on gpui-pre instead, turn off the
default features:

```toml
gpui-cn = { version = "0.1", default-features = false }
```

## Show a button

```rust
use gpui_cn::prelude::*;
use gpui_kit::*;

struct Hello;

impl Render for Hello {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().p_4().child(
            Button::new("save")
                .primary()
                .label("Save")
                .on_click(|_, window, cx| {
                    let _ = window.prompt(PromptLevel::Info, "Saved", None, &["OK"], cx);
                }),
        )
    }
}

fn main() {
    gpui_kit::application()
        .with_assets(gpui_kit::assets::Assets)
        .run(|cx| {
            gpui_kit::init(cx);
            gpui_cn::init(cx);
            gpui_kit::open_window(WindowOptions::default(), cx, |_, cx| cx.new(|_| Hello))
                .expect("failed to open window");
        });
}
```

`with_assets` registers the Lucide icons that gpui-kit bundles, which the
components draw with. The composer family (`Composer`, `PermissionMenu`,
`AttachmentTile`, `EffortMenu`, `StatusSelect`) also draws icons the default
set lacks, so register `gpui_cn::ComposerAssets` in its place. Call `gpui_kit::init` first. It sets up `gpui-base`.
Then call `gpui_cn::init`, which installs the theme and adds gpui-cn to
the `Root` that `gpui_kit::open_window` puts at the top of each window.
gpui-base's root moves focus on Tab and Shift-Tab and copies the selected
text on Cmd-C; gpui-cn paints the window background from the theme, sets
the rem size, and hosts the tooltip overlay.

## Theme

A `ThemeConfig` holds the inputs for one appearance: surface, ink, accent,
contrast, fonts, and semantic colors. `Theme` holds one config for light
and one for dark. Every token a component reads is derived from the active
config and copied onto `gpui_base::Theme`, so the scrollbars, inputs, and
resize handles from `gpui-base` use the same colors.

```rust
Theme::change(ThemeMode::Dark, cx);
Theme::set_ui_font_size(cx, px(18.));
Theme::update(cx, |theme| {
    theme.light.accent = gpui_cn::theme::hex("#0f766e");
    theme.reduce_motion = ReduceMotion::On;
    theme.pointer_cursors = true;
});
```

Components read tokens with `cx.theme()`. The tokens hold the colors,
radii, spacing, and typography from `gpui-base`, plus:

- `text_control`, `text_heading`, and `text_title`: 13px, 15px, and 24px
  text at the default UI font size.
- `metrics`: the heights and paddings of controls, the heights of rows,
  the title bar height, the space the window controls take, and the
  sidebar widths.

Control and row sizes scale with the UI font size. The title bar, the
window controls, and the sidebar widths stay in pixels. No component holds
a size of its own, so a theme that changes `metrics` changes every
component. `theme.touch`, on by default on iOS and Android, grows controls
and rows to the 44pt hit target of Apple's Human Interface Guidelines,
sets text to its body size, and turns hover states off.

## Using the terminal

The `ghostty` feature adds `gpui_cn::terminal`, a terminal on Ghostty's
libghostty-vt. It is off by default, so `cargo add gpui-cn` needs nothing
new.

```bash
cargo add gpui-cn --features ghostty
```

Building it needs [Zig](https://ziglang.org/download/) 0.17 on `PATH` (or
in `ZIG`). The first build downloads Ghostty's pinned source, about 4 MB,
and the Zig packages it fetches, about 30 MB, into Zig's cache. For an
offline build, set `GHOSTTY_SOURCE_DIR` to a Ghostty checkout or the
extracted tarball and `GHOSTTY_ZIG_SYSTEM_DIR` to the unpacked Zig
packages; `ghostty-vt-sys` describes both.

Everything else is decided at runtime: a local shell in a pty
(`TerminalState::local`, on desktop targets), a remote session or any
other byte stream (`StreamSource`), and parking of idle terminals, which
is on at 60 seconds and set through `ParkOptions`. The `gpui_cn::terminal`
documentation has a complete example, and the gallery's Terminal story
adds tabs, splits, menus and a leader key on top.

`gpui_cn::init` binds no terminal keys. A focused terminal sends every
key a program needs, Tab and Ctrl-C included. Copy, paste, scrolling and
font zoom are actions; install Ghostty's keys for them, or bind your own
in `terminal::KEY_CONTEXT`:

```rust
cx.bind_keys(gpui_cn::terminal::default_key_bindings());
```

## Gallery

The gallery shows every component in every variant, size, and state. It
is built from gpui-cn components, so it is also the first application that
uses the library.

```bash
git submodule update --init
cargo run
```

The Terminal story runs your shell through Ghostty's terminal engine, so
building the gallery needs [Zig](https://ziglang.org/download/) 0.17. Run
`mise install`; the build finds the repository version through mise even
when another `zig` is first on `PATH`. Without the submodule, the first
build downloads the pinned Ghostty source instead.
`cargo run --no-default-features --features gpui-fast` builds the gallery
without the story and without Zig. Without `--features gpui-fast`, it also
runs on gpui-pre.

Pick a story in the sidebar. Each story you pick is a page on the
navigation stack, and the arrows in the title bar walk back and forward
through them. Tab moves focus through the page and shows the focus ring.
Hover an icon button or a truncated label to see its tooltip. In a window
narrower than 768px the sidebar becomes a sheet over the page, as on a
phone.

| Key | Action |
| --- | --- |
| Cmd-, | Open Settings: appearance, reduced motion, pointer cursors, text size, code font, and the sidebar's collapse mode |
| Cmd-B | Hide or show the sidebar |
| Cmd-K | Open the component palette |
| Cmd-[ | Go back |
| Cmd-] | Go forward |

On Windows and Linux, the same actions use Ctrl-, Ctrl-B, Ctrl-K, Alt-Left, and
Alt-Right.

On macOS, the gallery can also render itself without a window and write
PNG files: the gallery, the sidebar collapsed off the canvas, the sidebar
as a rail, the settings page, and each story, once per appearance. Pass a
height to see a story that scrolls.

```bash
cargo run -p gpui-cn-story --features snapshot --bin snapshot -- snapshots
cargo run -p gpui-cn-story --features snapshot --bin snapshot -- snapshots 2400
```

## Performance

Two tools measure the gallery:

```bash
cargo run --release -p gpui-cn-story --features snapshot --bin bench
cargo run --release --features fps -- --exercise
```

`GPUI_CN_STARTUP_TRACE=1` makes the gallery print when the application
was built, when the window existed, and when the first frame drew, in
milliseconds since `main`. `bench` renders every page offscreen and prints what a full frame, an
idle frame, a wheel step, and the sidebar's slide cost, with the resident
memory before and after the slide. The `fps` feature adds the
[gpui-fps](https://crates.io/crates/gpui-fps) HUD, toggled with
Cmd-Shift-P or **View > Performance HUD**, which reads GPUI's own frame
trace. `--exercise` drives the gallery by itself (scroll, slide, next
story) so a profiler such as Instruments can watch it.

## Development

Every change must pass these commands:

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features
RUSTDOCFLAGS='-D warnings' cargo doc -p gpui-cn -p ghostty-vt -p ghostty-vt-sys --no-deps --all-features
cargo deny check licenses
```

`AGENTS.md` describes how the library is built and the rules a change
must follow.

The workspace also holds the terminal engine that gpui-cn's `ghostty`
feature uses: `ghostty-vt-sys` builds libghostty-vt from Ghostty's source
with Zig and links it statically, and `ghostty-vt` is its safe API. The
Ghostty pin is in `GHOSTTY.lock` and the `third_party/ghostty` submodule. To
move it, which also regenerates the bindings, the terminfo database, and
the shell integration scripts:

```bash
scripts/sync.sh <ghostty-commit>
```

The engine has two examples that need no GPUI:

```bash
cargo run -p ghostty-vt --example shell
cargo run -p ghostty-vt --example demo -- 'ls --color=always'
```

To release, set gpui-cn's version in `Cargo.toml`, commit, and push a tag
with the same version (`v0.5.0`). The `Release` workflow checks that the
tag matches, then publishes `ghostty-vt-sys`, `ghostty-vt` and `gpui-cn`
in that order, skipping any crate whose version is already on crates.io.
The engine crates keep their own versions, so they publish only when
bumped. Publishing uses crates.io Trusted Publishing, which cannot create
a crate: for the first release of a new crate, set a `CARGO_REGISTRY_TOKEN`
repository secret, which the workflow then uses, and remove it after. The
gallery crate is marked `publish = false`.

## License

Apache-2.0. Third-party assets:

- The Lucide icons come from the `gpui-kit-assets` crate under the ISC
  license.
- JetBrains Mono, used with the `jetbrains-mono` feature, comes from the
  `damascene-fonts-jetbrains-mono` crate under the SIL Open Font License
  1.1.
- The terminal engine ships Ghostty's terminfo database and shell
  integration scripts under Ghostty's MIT license (`third_party/LICENSE-GHOSTTY`).
  `third_party/README.md` lists the code the terminal is adapted from
  and the native libraries linked into libghostty-vt.
