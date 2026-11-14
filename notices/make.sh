#!/bin/sh
# Writes the notices Sandhill.app carries for the open-source Rust libraries
# built into it, into the folder given (default: notices/out):
#
#   sandhilld.txt     the daemon, for macOS and Linux servers
#   sandhill-app.txt  the app
#
# Needs cargo-about 0.9 (`cargo install cargo-about --locked --features cli`).
# Run before making the app bundle; .github/workflows/gpui-mac.yml does.
set -eu

repo=$(CDPATH='' cd -- "$(dirname -- "$0")/.." && pwd)
out=${1:-"$repo/notices/out"}
mkdir -p "$out"
for pair in "Cargo.toml:sandhilld.txt" "gpui/Cargo.toml:sandhill-app.txt"; do
    manifest=${pair%%:*}
    cargo about generate --locked --fail \
        --config "$repo/notices/about.toml" \
        --manifest-path "$repo/$manifest" \
        --output-file "$out/${pair#*:}" \
        "$repo/notices/about.hbs"
done
echo "notices written to $out"
