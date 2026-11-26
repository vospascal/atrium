#!/bin/sh
# Sample a stretch of an online video's audio into a WAV for `analyse`.
# Only the stretch is fetched (yt-dlp --download-sections), never the whole
# video. The WAV keeps the source's sample rate and both channels, so a
# binaural beat (one tone per ear) survives; online audio is lossy (YouTube
# about 128–160 kbps), fine for envelopes, beats, modulation and partials up
# to about 16 kHz. Check per clip that left and right are really different.
#
# Clips are other people's recordings: assets/clips/ is in .gitignore.
#
# Usage: sh scripts/clip.sh <url> <from> <to> [name]
#   <from>, <to>   seconds (90) or minutes:seconds (1:30) or h:m:s (1:01:30)
#   [name]         the file's name; default the video's id
# Writes assets/clips/<name>-<from>-<to>.wav (times in whole seconds), its
# left and right channels as mono files in assets/clips/channels/ (what the
# reference scenes play: the engine's recordings are mono), and prints the
# analyse line for it.
#
# Needs yt-dlp and ffmpeg: brew install yt-dlp ffmpeg

set -eu

usage() {
    sed -n '2,19p' "$0" | sed 's/^# \{0,1\}//'
    exit "$1"
}

[ $# -ge 3 ] && [ $# -le 4 ] || usage 1

url=$1
from=$2
to=$3
name=${4:-'%(id)s'}

for tool in yt-dlp ffmpeg; do
    if ! command -v "$tool" >/dev/null 2>&1; then
        echo "clip: $tool is not installed; run: brew install yt-dlp ffmpeg" >&2
        exit 1
    fi
done

# h:m:s, m:s or s (decimals allowed) → whole seconds, for the file name.
seconds() {
    echo "$1" | awk -F: '
        /^[0-9]+(\.[0-9]+)?(:[0-9]+(\.[0-9]+)?){0,2}$/ {
            total = 0
            for (i = 1; i <= NF; i++) total = total * 60 + $i
            printf "%d\n", total
            next
        }
        { exit 1 }'
}

from_seconds=$(seconds "$from") || { echo "clip: bad start time '$from'" >&2; exit 1; }
to_seconds=$(seconds "$to") || { echo "clip: bad end time '$to'" >&2; exit 1; }
if [ "$to_seconds" -le "$from_seconds" ]; then
    echo "clip: the end ($to) must come after the start ($from)" >&2
    exit 1
fi

root=$(cd "$(dirname "$0")/.." && pwd)
mkdir -p "$root/assets/clips"

path=$(yt-dlp \
    --no-playlist \
    --format bestaudio \
    --download-sections "*$from-$to" --force-keyframes-at-cuts \
    --extract-audio --audio-format wav \
    --output "$root/assets/clips/$name-$from_seconds-$to_seconds.%(ext)s" \
    --no-simulate --print after_move:filepath \
    "$url")

# The two channels apart, each a mono file, for the reference scenes.
mkdir -p "$root/assets/clips/channels"
stem=$(basename "$path" .wav)
ffmpeg -loglevel error -y -i "$path" \
    -filter_complex "channelsplit=channel_layout=stereo[left][right]" \
    -map "[left]" "$root/assets/clips/channels/$stem-left.wav" \
    -map "[right]" "$root/assets/clips/channels/$stem-right.wav"

echo "$path"
echo "cargo run --release --bin analyse -- \"${path#"$root"/}\" --picture"
