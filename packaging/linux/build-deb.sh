#!/bin/bash
# Build the app and its local daemon for the Ubuntu host, then make a .deb.
set -euo pipefail

repo=$(cd -- "$(dirname -- "$0")/../.." && pwd)
out=${1:-"$repo/target/deb"}
mkdir -p "$out"
out=$(cd -- "$out" && pwd)
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
stage="$work/sandhill"
arch=$(dpkg --print-architecture)
target=$(rustc -vV | awk '$1 == "host:" { print $2 }')
export SOURCE_DATE_EPOCH=${SOURCE_DATE_EPOCH:-$(git -C "$repo" log -1 --format=%ct)}

# Both workspaces have their own lockfile and target directory. Get the
# package version from the app's manifest, not from the release tag.
version=$(cd "$repo/app" && cargo metadata --locked --no-deps --format-version 1 \
    | jq -er '.packages[] | select(.name == "sandhill-app") | .version')
(cd "$repo" && cargo build --locked --release -p sandhill-daemon --target-dir "$repo/target")
(cd "$repo/app" && cargo build --locked --release --target-dir "$repo/app/target")

app_dir="$stage/usr/lib/sandhill"
docs="$stage/usr/share/doc/sandhill"
install -Dm755 "$repo/app/target/release/sandhill" "$app_dir/sandhill"
install -Dm755 "$repo/target/release/sandhilld" "$app_dir/sandhilld"
strip --strip-unneeded "$app_dir/sandhill" "$app_dir/sandhilld"
mkdir -p "$stage/usr/bin"
ln -s ../lib/sandhill/sandhill "$stage/usr/bin/sandhill"
install -Dm644 "$repo/packaging/linux/sandhill.desktop" "$stage/usr/share/applications/sandhill.desktop"

# The small crane has more readable detail at launcher sizes. Render the
# existing Sandhill artwork, rather than shipping a separate Linux icon.
for size in 16 32 48 64 128 256 512; do
    icon=source.svg
    if (( size <= 32 )); then icon=source-small.svg; fi
    dir="$stage/usr/share/icons/hicolor/${size}x${size}/apps"
    mkdir -p "$dir"
    rsvg-convert -w "$size" -h "$size" -o "$dir/sandhill.png" "$repo/app/assets/icon/$icon"
done

# Include what is in these binaries: Rust dependencies, embedded icons,
# the vendored GPUI extensions, and Ghostty's native terminal libraries.
# Unlike the Mac app, the Linux app has no bundled server downloads.
install -Dm644 "$repo/LICENSE" "$docs/copyright"
sh "$repo/notices/make.sh" "$docs/licenses" "$target"
icons="$repo/docs/design/ui/icons/vendor"
install -Dm644 "$icons/NOTICE.txt" "$docs/licenses/icons/NOTICE.txt"
install -Dm644 "$icons/gruvbox/LICENSE" "$docs/licenses/icons/Gruvbox-Material-LICENSE"
install -Dm644 "$icons/lucide/LICENSE" "$docs/licenses/icons/Lucide-LICENSE"
install -Dm644 "$icons/developer/LICENSE" "$docs/licenses/icons/Developer-Icons-LICENSE"
install -Dm644 "$icons/brands/oh-my-pi.LICENSE" "$docs/licenses/icons/Oh-My-Pi-LICENSE"
install -Dm644 "$icons/brands/pi.LICENSE" "$docs/licenses/icons/pi-LICENSE"
install -Dm644 "$repo/app/vendor/gpui-cn/LICENSE-APACHE" "$docs/licenses/gpui-cn/LICENSE-APACHE"
install -Dm644 "$repo/app/vendor/gpui-cn/LICENSE-MUXY" "$docs/licenses/gpui-cn/LICENSE-MUXY"
install -Dm644 "$repo/app/vendor/gpui-component/LICENSE-APACHE" "$docs/licenses/gpui-component/LICENSE-APACHE"
install -Dm644 -t "$docs/licenses/libghostty-vt" "$repo"/notices/libghostty-vt/*-LICENSE*

# dpkg-shlibdeps maps the two binaries' ELF references to package names and
# minimum versions. It expects a Debian source control file in its cwd.
mkdir -p "$work/debian" "$stage/DEBIAN"
printf 'Source: sandhill\n' > "$work/debian/control"
depends=$(cd "$work" && dpkg-shlibdeps -O "$app_dir/sandhill" "$app_dir/sandhilld")
depends=${depends#shlibs:Depends=}
# Vulkan is loaded at runtime, the chooser uses an XDG portal over D-Bus,
# and WebKitGTK needs an X display (provided by XWayland on Ubuntu GNOME).
cat > "$stage/DEBIAN/control" <<EOF
Package: sandhill
Version: $version
Architecture: $arch
Maintainer: Sandhill <26470627+dotblot@users.noreply.github.com>
Section: devel
Priority: optional
Installed-Size: $(du -sk --apparent-size "$stage" | cut -f1)
Depends: $depends, libvulkan1, mesa-vulkan-drivers | vulkan-icd, xdg-desktop-portal, xdg-desktop-portal-gtk | xdg-desktop-portal-backend, xwayland | xserver-xorg, xdg-utils
Recommends: git, openssh-client
Homepage: https://github.com/dotblot/sandhill
Description: R and Python IDE for bioinformatics
 Sandhill opens local and remote projects, runs R and Python notebooks and
 scripts, and shows their plots, variables and tables alongside the code.
EOF

deb="sandhill_${version}_${arch}.deb"
dpkg-deb --root-owner-group --build "$stage" "$out/$deb"
(cd "$out" && sha256sum "$deb" > "$deb.sha256")
