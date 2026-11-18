# Sandhill on Ubuntu

Status: approved by Huan (2026-11-18).

## What Huan asked for
- A Linux version of Sandhill, starting with Ubuntu: one Ubuntu first,
  other Linux systems later, if ever.
- All of it planned now: building, running, previews, an install file,
  and a GitHub check.
- Huan's Ubuntu 26.04 machine (`ubuntu-lab`) is the test machine; I
  reach it over SSH from the Mac.

## What is true today
- **The daemon already works on Linux.** GitHub tests it on Ubuntu 24.04
  with Python, IRkernel and Ark on every push, and servers run it.
- **The window was written with Linux in mind but has never been run
  there.** Of the app's Mac-only code, most already has a Linux branch:
  - shortcuts use Ctrl where the Mac uses ⌘ (`main.rs`);
  - settings and caches go to the usual Linux folders (`prefs.rs`,
    `media/cache.rs`);
  - glass is off, and its switch hidden, off the Mac (`look.rs`,
    `general.rs`);
  - names read "This computer" and "Show in folder" instead of "This Mac"
    and "Reveal in Finder";
  - choosing files and folders uses GPUI's own dialog, which on Linux is
    Ubuntu's standard file chooser.
- **Gaps:**
  - **Previews:** PDFs and images open in Apple's viewers
    (`editor/media/native.rs`, PDFKit). Off the Mac they show an error.
  - **Window buttons:** Sandhill draws its own title bar. On the Mac the
    system adds the three coloured buttons; on Linux nobody draws close,
    minimise or maximise, so the window may have none.
  - **Quit:** the question about running kernels on quit hooks into the
    Mac's Quit menu (`shell/app_window/quit.rs`); on Linux that hook does
    nothing yet.
  - **Uninstall** in Welcome removes the Mac app bundle
    (`welcome/app_uninstall.rs`); on Linux a package manager does that.
  - **Default code font:** the Linux branch exists but is untested.
- **The app is only checked on the Mac.** GitHub never builds it on
  Linux, so some build errors are likely.
- **`ubuntu-lab`:**
  - Ubuntu 26.04.1, 32 cores, 123 GB memory, an NVIDIA RTX 4080 SUPER and
    an AMD graphics chip;
  - a GNOME desktop on Wayland, the newer Linux display system;
  - Rust, Python and R are installed. Zig and the libraries GPUI and the
    web view need are not;
  - automatic sleep is off;
  - only a close button is shown, on the right (its `button-layout`
    setting is `appmenu:close`);
  - the desktop's fonts are Ubuntu Sans for the interface and Ubuntu Sans
    Mono for code (GNOME's settings; fontconfig's own default for code is
    DejaVu Sans Mono).
- **Found while building (2026-11-18):** the web view (WebKitGTK) works
  only on X11, so on Ubuntu's Wayland desktop Sandhill runs through
  XWayland, the X11 layer Ubuntu includes. It looks and behaves as a normal
  window; the difference shows only with fractional display scaling, where
  X11 apps can look slightly soft.

## The change
1. **Build it on `ubuntu-lab`.**
   - Install what the build needs (one command for Huan, see below).
   - Fix whatever stops the app from compiling, without changing the Mac
     version.
   - The app's checks (`cargo fmt`, `clippy`, `cargo test`) pass there.
2. **Run it and make it work right on Ubuntu:**
   - **Window buttons:** Sandhill draws close, minimise and maximise in
     its title bar, in Ubuntu's style, on the right as Ubuntu puts them.
     All three always show, whatever the desktop's own setting says
     (Huan, 2026-11-18). Double-clicking the title bar maximises, as
     Ubuntu does.
   - **Colours:** solid, as the Mac's look is with glass turned off;
     light and dark follow Ubuntu's own setting.
   - **Fonts:** the interface uses Ubuntu's interface font. Code uses the
     desktop's default monospaced font, named "System (Ubuntu Sans
     Mono)" in General. The rest of the font list works as on the Mac.
   - **Shortcuts:** check every Ctrl shortcut; fix any that GNOME already
     uses.
   - **Quit:** Ctrl+Q and closing the last window ask about running
     kernels, as Quit does on the Mac.
   - **Uninstall** in Welcome is replaced on Linux by the command that
     removes the package.
   - **Kernels:** Ark installs on this computer the way it already does
     on servers.
3. **Previews:**
   - **Images** (PNG, JPEG, GIF, WebP, BMP, TIFF, ICO, SVG) are drawn by
     GPUI itself.
   - **PDFs** open in the built-in web view (WebKitGTK on Linux) with
     PDF.js, Mozilla's PDF viewer (Apache 2.0 licence, so free to ship
     with its notice). WebKitGTK has no PDF viewer of its own.
   - The toolbar works as on the Mac: Fit, zoom, pages, the hand.
4. **An install file for Ubuntu (`.deb`):**
   - It contains the app, `sandhilld`, the icon, a menu entry and the
     licence notices.
   - It names the Ubuntu packages it needs, so installing it brings them
     in.
   - It is built on Ubuntu 24.04, so it installs on 24.04 and 26.04.
   - Releases on GitHub carry it next to the Mac DMG.
5. **A GitHub check:** a Linux app job that builds and tests the app.
   - It runs at the same time as the Mac jobs, so a push takes about as
     long as today.
   - The `.deb` is built only for releases, not on every push.
6. **Records:**
   - the Linux decisions go into `docs/decisions/` with their date;
   - Linux build notes go into `app/README.md` and `AGENTS.md`, replacing
     the old X11 note;
   - this plan moves to `docs/design/done/`.

## How I'll see the screen
I can't look at Ubuntu's screen over SSH, and `ubuntu-lab` need not stay
unlocked.
- **For my own checks** I start Sandhill on a hidden screen on
  `ubuntu-lab` and take screenshots there. That doesn't touch Huan's
  desktop. It shows layout, colours and text, but not how the real
  desktop draws the window.
- **Huan checks the real window** on the Ubuntu screen after steps 2, 3
  and 4, as he does with the Mac app.

## What Huan does
- Run one install command on `ubuntu-lab` when step 1 starts. Installing
  system packages needs administrator rights (`sudo`), which I won't use
  without asking; I'll give the exact line.
- Keep automatic sleep off (it is now), so the machine answers SSH.
- Look at Sandhill on the Ubuntu screen at the three check points above.

## Huan's answers (2026-11-18)
- **Window buttons:** always close, minimise and maximise.
- **Other Linux systems** (Fedora, Arch, other desktops): later.

## Checks
- The workspace and `app/` checks pass on `ubuntu-lab` and on the Mac.
- The usual tests for any new preview code.
- Screenshots on Ubuntu, light and dark: Welcome, a notebook, the
  Console, the terminal, an image and a PDF preview, the quit question.
- Installing the `.deb` on `ubuntu-lab`, opening a project, running
  Python and R cells, then removing it.
