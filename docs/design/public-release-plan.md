# First public release on GitHub (plan, approved 2026-11-18)

Huan wants anyone on GitHub to be able to download and use Sandhill
(2026-11-18). The repository is already public (`dotblot/sandhill`,
GPL-3.0-or-later) and has no releases. The version stays 0.1.0 (Huan,
2026-11-13); this release is marked as a preview.

## Where things stand
- The Mac app is built for Apple-silicon Macs only (`macos-15` runner,
  `Sandhill-macos-arm64.zip`, kept 7 days as a build artifact). It is
  signed ad hoc (`codesign --sign -`), so on another Mac, macOS says it
  cannot check the app.
- Servers: Ubuntu 24.04 is tested; the daemon ships for x86_64 and aarch64
  Linux (static musl builds); Ark needs glibc.
- Agents: Claude Code and Codex only (the others paused, 2026-11-18).
- Open bugs: kernel shutdown sometimes fails with "kernel did not stop
  gracefully" (cause unknown, the message now says more); conversation
  titles cut at the wrong place (`title_of` in `app/src/agents/chat.rs`
  doesn't stop at a colon); occasional dropped frame scrolling a very large
  notebook; start-up memory 105 → 137 MB, not looked into.
- Rights: the README says the Posit Assistant and Antigravity marks have
  no public-release permission; the font author question is still open.

## Steps
1. **Green and tried.** Push the waiting test fix; CI and the Mac app
   build green. Huan uses the latest build for a few days.
2. **Bugs.** Fix the conversation title bug. Find the kernel shutdown
   cause from the next failure's message, or fix it from a reproduction.
3. **Rights.** Per Huan's choice below: get permission for the Posit
   Assistant and Antigravity marks, or leave them out of the shipped app
   (their sources stay in the repository). Settle the font author
   question.
4. **Apple signing** (if Huan chooses it): Huan joins the Apple Developer
   Program and makes a Developer ID certificate; the certificate and an
   App Store Connect API key go into GitHub secrets. The release build
   signs with the hardened runtime, has Apple notarize it, and staples
   the ticket, so the app opens without warnings.
5. **Release workflow.** A tag `v0.1.0-preview.N` builds the app and the
   Linux daemons as the Mac app workflow does, signs it (step 4), makes
   `Sandhill-0.1.0-preview.N-macos-arm64.dmg` and its SHA-256, and creates
   a GitHub release marked as a pre-release, with notes Huan approves.
6. **README: Install.** What to download; needs an Apple-silicon Mac;
   how to open it (and, unsigned, how to get past the warning); R and
   Python kernels to install; servers (Ubuntu 24.04 tested, other 64-bit
   Linux expected); agents need the user's own Claude or ChatGPT plan;
   no automatic updates; what Sandhill sends where (nothing of its own;
   agents and connectors talk to their services), checked in the code
   before it is written down.
7. **Bug reports.** A GitHub issue form asking for the Sandhill version,
   macOS version, local or server (and its Linux), and what happened.

## Huan's choices (2026-11-18)
- Apple signing: none. The release is unsigned; the README explains how
  to open it (step 4 is dropped).
- The Posit Assistant and Antigravity marks are left out of the shipped
  app for now; their sources stay in the repository.
- Apple-silicon Macs only.
- Fixed before the release: the conversation title bug and the kernel
  shutdown failure. Start-up memory and the rare dropped frame wait.
- Also before the release: the UI consistency work
  (`docs/design/ui-consistency-plan.md`), paused after step 1.
