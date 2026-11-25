#!/bin/bash
# Builds the test applications into ~/TwoWin.app and ~/Alone.app. Run it on the Mac.
#
# swiftc stamps its output with the newest macOS by default, and an app bundle that holds such
# a binary fails to open with -10825, so the target is explicit.
set -eu
here=$(cd "$(dirname "$0")" && pwd)
bundle() {
    local name=$1 source=$2 app=$HOME/$1.app
    rm -rf "$app"
    mkdir -p "$app/Contents/MacOS"
    swiftc -O -target "$(uname -m)-apple-macos11" "$source" -o "$app/Contents/MacOS/$name"
    cat > "$app/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>CFBundleExecutable</key><string>$name</string>
<key>CFBundleIdentifier</key><string>sh.atuin.desktop-capture-test.$name</string>
<key>CFBundleName</key><string>$name</string>
<key>CFBundlePackageType</key><string>APPL</string>
<key>LSMinimumSystemVersion</key><string>11.0</string>
</dict></plist>
PLIST
    codesign -s - --force "$app" 2> /dev/null
    echo "built $app"
}
bundle TwoWin "$here/twowin.swift"
bundle Alone "$here/alone.swift"
