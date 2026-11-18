#!/bin/sh
# Writes the third-party notices Sandhill ships into the folder given
# (default: notices/out). Optionally takes a Cargo target triple after it.
#
#   sandhilld.txt     the daemon, for macOS and Linux servers
#   sandhill-app.txt  the app
#   pdfjs/            the bundled viewer (Linux app)
#
# Needs cargo-about 0.9 (`cargo install cargo-about --locked --features cli`).
# Run before making the app bundle; .github/workflows/mac-app.yml does.
set -eu

repo=$(CDPATH='' cd -- "$(dirname -- "$0")/.." && pwd)
out=${1:-"$repo/notices/out"}
target=${2:-}
set --
if [ -n "$target" ]; then
    set -- --target "$target"
fi
mkdir -p "$out"
for pair in "Cargo.toml:sandhilld.txt" "app/Cargo.toml:sandhill-app.txt"; do
    manifest=${pair%%:*}
    cargo about generate --locked --fail "$@" \
        --config "$repo/notices/about.toml" \
        --manifest-path "$repo/$manifest" \
        --output-file "$out/${pair#*:}" \
        "$repo/notices/about.hbs"
done

case "$target" in
    *-linux-*)
        src="$repo/app/vendor/pdfjs"
        pdfjs="$out/pdfjs"
        mkdir -p "$pdfjs/web/cmaps" "$pdfjs/web/iccs" "$pdfjs/web/standard_fonts" "$pdfjs/web/wasm"
        cp "$src/LICENSE" "$src/SANDHILL.md" "$pdfjs/"
        cp "$src/web/cmaps/LICENSE" "$pdfjs/web/cmaps/"
        cp "$src/web/iccs/LICENSE" "$pdfjs/web/iccs/"
        cp "$src/web/standard_fonts/"LICENSE_* "$pdfjs/web/standard_fonts/"
        cp "$src/web/wasm/"LICENSE_* "$pdfjs/web/wasm/"
        ;;
esac
echo "notices written to $out"
