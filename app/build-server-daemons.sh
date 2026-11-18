#!/bin/sh
# Build the matching Linux daemons and bundle pinned Python language servers.
# Run before signing the app; this does not connect to or change any server.
# Pass --linux-only after the output directory to omit the Mac language servers.
set -eu

repo=$(CDPATH='' cd -- "$(dirname -- "$0")/.." && pwd)
destination=${1:-"$repo/app/target/debug/Sandhill.app/Contents/Resources/daemons"}
mkdir -p "$destination"

for target in x86_64-unknown-linux-musl aarch64-unknown-linux-musl; do
    cargo zigbuild --manifest-path "$repo/Cargo.toml" --locked --release \
        --package sandhill-daemon --bin sandhilld --target "$target"
    cp "$repo/target/$target/release/sandhilld" "$destination/sandhilld-$target"
    chmod 755 "$destination/sandhilld-$target"
done

# Official immutable Astral releases. SHA-256 values are from each release's
# sha256.sum; verify the archive before extracting or copying executable bytes.
# https://github.com/astral-sh/ty/releases/download/0.0.102/sha256.sum
# https://github.com/astral-sh/ruff/releases/download/0.17.5/sha256.sum
stage=$(mktemp -d)
trap 'rm -rf "$stage"' EXIT HUP INT TERM

hash() {
    if command -v sha256sum >/dev/null 2>&1; then
        sha256sum "$1"
    else
        shasum -a 256 "$1"
    fi
}

payload() {
    program=$1
    version=$2
    target=$3
    checksum=$4
    archive="$program-$target.tar.gz"
    url="https://github.com/astral-sh/$program/releases/download/$version/$archive"
    curl --fail --location --silent --show-error "$url" -o "$stage/$archive"
    actual=$(hash "$stage/$archive")
    [ "${actual%% *}" = "$checksum" ] || {
        echo "SHA-256 mismatch: $url" >&2
        exit 1
    }
    unpack="$stage/$program-$target"
    mkdir "$unpack"
    # Astral archives contain one top-level directory (as used by their
    # official installer); do not depend on its musl/static spelling.
    tar -xzf "$stage/$archive" --strip-components 1 -C "$unpack"
    source="$unpack"
    cp "$source/$program" "$destination/$program-$target"
    chmod 755 "$destination/$program-$target"
    binary=$(hash "$destination/$program-$target")
    printf '{"program":"%s","version":"%s","url":"%s","archive_sha256":"%s","binary_sha256":"%s"}\n' \
        "$program" "$version" "$url" "$checksum" "${binary%% *}" \
        > "$destination/$program-$target.source.json"
    # The executable archives may omit the license; always retain the complete
    # notice from the exact release tag, including Ruff's incorporated notices.
    if [ ! -f "$stage/$program-LICENSE" ]; then
        curl --fail --location --silent --show-error \
            "https://raw.githubusercontent.com/astral-sh/$program/$version/LICENSE" \
            -o "$stage/$program-LICENSE"
    fi
    mkdir -p "$destination/$program-$target.notices"
    cp "$stage/$program-LICENSE" "$destination/$program-$target.notices/LICENSE"
    # Retain upstream notices distributed alongside each executable.
    for notice in "$source"/LICENSE* "$source"/NOTICE* "$source"/README*; do
        [ -f "$notice" ] || continue
        mkdir -p "$destination/$program-$target.notices"
        cp "$notice" "$destination/$program-$target.notices/"
    done
}

payload ty 0.0.102 x86_64-unknown-linux-musl f6b2200892c1271fd98839e73866a38326449263fdc201087a4266db9ebbd7a0
payload ty 0.0.102 aarch64-unknown-linux-musl a5d2a5d8d14d132309ca17256fb9e28822c6be7650406f6a4e12af79ab3bff94
if [ "${2:-}" != --linux-only ]; then
    payload ty 0.0.102 aarch64-apple-darwin da0fead6652ab9e5b1837e0ed057123e478ffc022292fce041e6b4513a63b6b7
fi
payload ruff 0.17.5 x86_64-unknown-linux-musl dc94b7e225b8910fb6f55298ada32d00e356dbd195a5ffb647847f388cb54f84
payload ruff 0.17.5 aarch64-unknown-linux-musl 98f5ee4cf0cd2ad122ef61bbcce236782e5ac3cd00122dcf458ce19488863428
if [ "${2:-}" != --linux-only ]; then
    payload ruff 0.17.5 aarch64-apple-darwin 4ffadabec5c5fee1ddb2eecac0787ffe6fa18c665b73da9258b84211f40c5829
fi
