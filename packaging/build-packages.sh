#!/usr/bin/env bash
# Packs the release build into what a release offers: an archive, a Debian
# package and an AppImage, in dist/. Run after `cargo build --release`; the
# release workflow does exactly this, then signs the files.
#
# Usage: packaging/build-packages.sh VERSION
set -euo pipefail
umask 022
cd "$(dirname "$0")/.."

version=${1:?usage: packaging/build-packages.sh VERSION}
arch=$(uname -m)
case $arch in
    x86_64) deb_arch=amd64 ;;
    aarch64) deb_arch=arm64 ;;
    *) echo "unsupported architecture $arch" >&2; exit 1 ;;
esac
bin=target/release/oxshot
[ -x "$bin" ] || { echo "build first: cargo build --release" >&2; exit 1; }

# License files go along when there are any.
licenses=()
for file in LICENSE LICENSE-MIT LICENSE-APACHE; do
    [ -f "$file" ] && licenses+=("$file")
done

rm -rf dist
mkdir -p dist
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

# Puts the icons into the hicolor theme under $1.
icons() {
    for png in assets/brand/png/oxshot-mark-*.png; do
        size=$(basename "$png" .png); size=${size#oxshot-mark-}
        case $size in *[!0-9]*) continue ;; esac
        install -Dm 644 "$png" "$1/${size}x${size}/apps/oxshot.png"
    done
    install -Dm 644 assets/brand/oxshot-mark.svg "$1/scalable/apps/oxshot.svg"
}

# --- Archive: the program, for "Install for all users" and updates.
name=oxshot-$version-$arch-linux
mkdir "$work/$name"
cp "$bin" README.md ${licenses[@]+"${licenses[@]}"} "$work/$name/"
strip "$work/$name/oxshot"
tar -C "$work" -czf "dist/$name.tar.gz" "$name"

# --- Debian package.
deb=$work/deb
app=/usr/bin/oxshot
mkdir -p "$deb/DEBIAN" "$deb/usr/bin" "$deb/usr/share/applications" "$deb/usr/share/polkit-1/actions" \
    "$deb/usr/share/doc/oxshot"
install -m 755 "$work/$name/oxshot" "$deb$app"
"$deb$app" --desktop-entry "$app" > "$deb/usr/share/applications/oxshot.desktop"
grep -q '^\[Desktop Entry\]' "$deb/usr/share/applications/oxshot.desktop" \
    || { echo "oxshot --desktop-entry printed no desktop file" >&2; exit 1; }
sed "s|@APP@|$app|g" packaging/io.github.srcmkr.oxshot.policy > "$deb/usr/share/polkit-1/actions/io.github.srcmkr.oxshot.policy"
icons "$deb/usr/share/icons/hicolor"
{
    echo "Format: https://www.debian.org/doc/packaging-manuals/copyright-format/1.0/"
    echo "Upstream-Name: OxShot"
    echo "Source: https://github.com/srcmkr/OxShot"
    echo
    echo "Files: *"
    echo "Copyright: srcmkr"
    echo "License: MIT or Apache-2.0"
    for file in ${licenses[@]+"${licenses[@]}"}; do
        echo
        cat "$file"
    done
} > "$deb/usr/share/doc/oxshot/copyright"
chmod 644 "$deb"/usr/share/applications/* "$deb"/usr/share/polkit-1/actions/* "$deb/usr/share/doc/oxshot/copyright"

cat > "$deb/DEBIAN/control" <<CONTROL
Package: oxshot
Version: $version
Section: graphics
Priority: optional
Architecture: $deb_arch
Maintainer: srcmkr <srcmkr@users.noreply.github.com>
Homepage: https://github.com/srcmkr/OxShot
Depends: libc6 (>= 2.35), libgcc-s1, libxkbcommon0, libxkbcommon-x11-0, libxcb1, libfontconfig1, libwayland-client0, libvulkan1
Recommends: pkexec | policykit-1, xdg-utils
Installed-Size: $(du -sk "$deb" | cut -f1)
Description: Fast screenshots with a Screenpresso-style annotation editor
 OxShot captures a region, a window or whole monitors on X11 in a few
 milliseconds from a frozen screen, with a pixel loupe, delayed capture and
 a screen color picker. Every capture is saved and put on the clipboard as
 image, file and path at once.
 .
 The editor keeps annotations as editable objects: arrows, shapes, text,
 callouts, numbered steps, highlighter, blur and pixelate, magnifier, crop,
 cut-out and frame effects. A history shows all captures as a grid. OxShot
 runs in the background with a tray icon and global hotkeys.
CONTROL
dpkg-deb --root-owner-group -Zxz --build "$deb" "dist/oxshot_${version}_${deb_arch}.deb" >/dev/null

# --- AppImage: one file to download and run.
appdir=$work/OxShot.AppDir
mkdir -p "$appdir/usr/bin"
install -m 755 "$work/$name/oxshot" "$appdir/usr/bin/"
cat > "$appdir/AppRun" <<'APPRUN'
#!/bin/sh
here=$(dirname "$(readlink -f "$0")")
exec "$here/usr/bin/oxshot" "$@"
APPRUN
chmod 755 "$appdir/AppRun"
"$appdir/usr/bin/oxshot" --desktop-entry oxshot > "$appdir/oxshot.desktop"
cp assets/brand/oxshot-mark.svg "$appdir/oxshot.svg"
ln -s oxshot.svg "$appdir/.DirIcon"
icons "$appdir/usr/share/icons/hicolor"

tool=${APPIMAGETOOL:-$work/appimagetool}
if [ ! -x "$tool" ]; then
    curl -sSfL -o "$tool" "https://github.com/AppImage/appimagetool/releases/download/1.9.1/appimagetool-$arch.AppImage"
    chmod 755 "$tool"
fi
log=$work/appimagetool.log
ARCH=$arch "$tool" --appimage-extract-and-run --no-appstream "$appdir" "dist/OxShot-$version-$arch.AppImage" >"$log" 2>&1 \
    || { cat "$log" >&2; echo "appimagetool failed" >&2; exit 1; }

# The same under names without the version, so links to the latest release
# (README) always work.
cp "dist/oxshot_${version}_${deb_arch}.deb" "dist/oxshot_${deb_arch}.deb"
cp "dist/OxShot-$version-$arch.AppImage" "dist/OxShot-$arch.AppImage"

( cd dist && sha256sum -- * > SHA256SUMS )
ls -l dist
