#!/bin/sh
# AirPods head tracker → OpenTrack UDP on 127.0.0.1:4242.
# Normally started for you by the engine:
#   cargo run --release -- --head-tracking=airpods scenes/frog.yaml
# Run it by hand only to build it or check the sensor (probe / rebuild below).
#
# Works with any Apple headphones that have head tracking (AirPods Pro, AirPods
# 3rd/4th generation, AirPods Max, some Beats) on macOS 14 or later. Built from
# scripts/airpods-head-tracker.swift into target/airpods-head-tracker/ and
# wrapped as AirPodsBridge.app: macOS only gives headphone motion to an app
# bundle that says why it wants it, and asks you once to allow it.
#
# Usage: sh scripts/airpods-head-tracker.sh            run until Ctrl+C
#        sh scripts/airpods-head-tracker.sh probe      check motion arrives
#        sh scripts/airpods-head-tracker.sh rebuild    force a fresh build

set -eu
REPOSITORY_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
SOURCE="$REPOSITORY_ROOT/scripts/airpods-head-tracker.swift"
WORK="$REPOSITORY_ROOT/target/airpods-head-tracker"
APP="$WORK/AirPodsBridge.app"
BINARY="$APP/Contents/MacOS/AirPodsBridge"
COMMAND="${1:-bridge}"

build() {
    echo "Building AirPodsBridge into $WORK ..."
    rm -rf "$APP"
    mkdir -p "$APP/Contents/MacOS"
    xcrun swiftc -O -swift-version 5 -o "$BINARY" "$SOURCE"
    cat > "$APP/Contents/Info.plist" <<'PLIST'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>CFBundleIdentifier</key><string>local.atrium.airpods-bridge</string>
<key>CFBundleName</key><string>AirPodsBridge</string>
<key>CFBundleExecutable</key><string>AirPodsBridge</string>
<key>CFBundlePackageType</key><string>APPL</string>
<key>CFBundleVersion</key><string>1</string>
<key>LSUIElement</key><true/>
<key>NSMotionUsageDescription</key><string>Turns the sound in Atrium with your head.</string>
</dict></plist>
PLIST
    codesign -s - -f --deep "$APP" 2>/dev/null
    echo "Built $APP"
}

# Rebuild when missing or when the Swift source is newer than the build.
if [ "$COMMAND" = "rebuild" ] || [ ! -x "$BINARY" ] || [ "$SOURCE" -nt "$BINARY" ]; then
    build
    [ "$COMMAND" = "rebuild" ] && exit 0
fi

# The log lives in TMPDIR: `open --stdout` into the project folder fails with
# LaunchServices error -10810 (the launched app may not write there).
LOG="${TMPDIR:-/tmp}/atrium-airpods-bridge.log"
: > "$LOG"
if [ "$COMMAND" = "probe" ]; then
    open -n -W --stdout "$LOG" --stderr "$LOG" "$APP" --args probe
    cat "$LOG"
    exit 0
fi

echo "Streaming AirPods head tracking to 127.0.0.1:4242 (log: $LOG). Ctrl+C to stop."
open -n --stdout "$LOG" --stderr "$LOG" "$APP" --args bridge --port 4242
trap 'pkill -f "$BINARY" 2>/dev/null; exit 0' INT TERM
tail -f "$LOG" || true
