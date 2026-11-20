#!/bin/sh
# Sony headphone head tracker → OpenTrack UDP on 127.0.0.1:4242.
# Normally started for you by the engine:
#   cargo run --release -- --head-tracking=sony scenes/frog.yaml
# Run it by hand only to build it or check the sensor (probe / rebuild below).
#
# Uses NicholasSlattery/sony-head-tracker (MIT), pinned below, built once into
# target/sony-head-tracker/ and wrapped as SonyBridge.app so macOS gives it its
# own Input Monitoring permission (a bare CLI started from an editor or agent
# inherits that app's permissions, which usually lack it).
#
# Headphone setup that is REQUIRED (measured 2026-11-17 on a WH-1000XM6):
#   - Sony Sound Connect app: Bluetooth Connection Quality = "Prioritize Stable
#     Connection". In "Prioritize Sound Quality" the headphones drop the HID
#     service and no samples arrive (Sony: support article 00352716).
#   - After changing that or updating firmware: Forget the headphones on the
#     Mac and pair again, or macOS never exposes the sensor.
#   - First run: System Settings > Privacy & Security > Input Monitoring →
#     enable SonyBridge (the script prints the path to add).
#
# Usage: sh scripts/sony-head-tracker.sh            run until Ctrl+C
#        sh scripts/sony-head-tracker.sh probe      check the sensor is visible
#        sh scripts/sony-head-tracker.sh rebuild    force a fresh build

set -eu
REPOSITORY_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
WORK="$REPOSITORY_ROOT/target/sony-head-tracker"
UPSTREAM="https://github.com/NicholasSlattery/sony-head-tracker.git"
PINNED_COMMIT="602bde7"
APP="$WORK/SonyBridge.app"
COMMAND="${1:-bridge}"

build() {
    echo "Building sony-head-tracker ($PINNED_COMMIT) into $WORK ..."
    rm -rf "$WORK/source" "$WORK/build" "$APP"
    mkdir -p "$WORK"
    git clone -q "$UPSTREAM" "$WORK/source"
    git -C "$WORK/source" checkout -q "$PINNED_COMMIT"
    CMAKE="$(command -v cmake || true)"
    if [ -z "$CMAKE" ]; then
        python3 -m venv "$WORK/venv"
        "$WORK/venv/bin/pip" install -q cmake
        CMAKE="$WORK/venv/bin/cmake"
    fi
    "$CMAKE" -S "$WORK/source" -B "$WORK/build" -DCMAKE_BUILD_TYPE=Release >/dev/null
    "$CMAKE" --build "$WORK/build" --target sony-head-tracker-macos --parallel >/dev/null
    mkdir -p "$APP/Contents/MacOS"
    cp "$WORK/build/sony-head-tracker-macos" "$APP/Contents/MacOS/SonyBridge"
    cat > "$APP/Contents/Info.plist" <<'PLIST'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>CFBundleIdentifier</key><string>local.atrium.sony-bridge</string>
<key>CFBundleName</key><string>SonyBridge</string>
<key>CFBundleExecutable</key><string>SonyBridge</string>
<key>CFBundlePackageType</key><string>APPL</string>
<key>CFBundleVersion</key><string>1</string>
<key>LSUIElement</key><true/>
<key>NSBluetoothAlwaysUsageDescription</key><string>Reconnects the Sony headphones' head tracker for Atrium.</string>
</dict></plist>
PLIST
    codesign -s - -f --deep "$APP" 2>/dev/null
    echo "Built $APP"
    echo "If this is the first build: enable SonyBridge under System Settings >"
    echo "Privacy & Security > Input Monitoring (add it with + if not listed)."
}

if [ "$COMMAND" = "rebuild" ] || [ ! -x "$APP/Contents/MacOS/SonyBridge" ]; then
    build
    [ "$COMMAND" = "rebuild" ] && exit 0
fi

# The log lives in TMPDIR: `open --stdout` into the project folder fails with
# LaunchServices error -10810 (the launched app may not write there).
LOG="${TMPDIR:-/tmp}/atrium-sony-bridge.log"
: > "$LOG"
if [ "$COMMAND" = "probe" ]; then
    open -n -W --stdout "$LOG" --stderr "$LOG" "$APP" --args probe
    cat "$LOG"
    exit 0
fi

echo "Streaming head tracking to 127.0.0.1:4242 (log: $LOG). Ctrl+C to stop."
open -n --stdout "$LOG" --stderr "$LOG" "$APP" --args bridge --port 4242
trap 'pkill -f "$APP/Contents/MacOS/SonyBridge" 2>/dev/null; exit 0' INT TERM
tail -f "$LOG" | grep --line-buffered -v -E "^YPR|^\[[0-9:.]+\] INFO" || true
