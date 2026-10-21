#!/usr/bin/env bash
# Builds dist/Jiggler.app (release, signed).
#
#   scripts/bundle.sh             build the app into dist/
#   scripts/bundle.sh --install   also copy it to /Applications (replacing any old copy)
#
# Signing: uses $SIGN_IDENTITY if set (e.g. "Developer ID Application: Name (TEAMID)"),
# otherwise ad-hoc. Ad-hoc builds get a new identity on every rebuild, so macOS treats
# each rebuild as a new app and Accessibility access must be granted again.
set -euo pipefail

cd "$(dirname "$0")/.."

APP_NAME="Jiggler"
BUNDLE_ID="${BUNDLE_ID:-com.jiggler.app}"
VERSION="$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)"
BUILD="${BUILD_NUMBER:-1}"
APP="dist/$APP_NAME.app"
export MACOSX_DEPLOYMENT_TARGET=11.0

cargo build --release

if [[ ! -f assets/AppIcon.icns ]]; then
    swift scripts/make_icon.swift .
fi

rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
cp target/release/jiggler "$APP/Contents/MacOS/jiggler"
cp assets/AppIcon.icns "$APP/Contents/Resources/AppIcon.icns"
sed -e "s/__BUNDLE_ID__/$BUNDLE_ID/" -e "s/__VERSION__/$VERSION/" -e "s/__BUILD__/$BUILD/" assets/Info.plist \
    > "$APP/Contents/Info.plist"
plutil -lint -s "$APP/Contents/Info.plist"

codesign --force --options runtime --timestamp=none \
    --sign "${SIGN_IDENTITY:--}" --identifier "$BUNDLE_ID" "$APP"
codesign --verify --strict "$APP"
echo "Built $APP ($VERSION build $BUILD, signed with ${SIGN_IDENTITY:-ad-hoc identity})"

if [[ "${1:-}" == "--install" ]]; then
    osascript -e "quit app \"$APP_NAME\"" >/dev/null 2>&1 || true
    rm -rf "/Applications/$APP_NAME.app"
    cp -R "$APP" /Applications/
    echo "Installed /Applications/$APP_NAME.app"
fi
