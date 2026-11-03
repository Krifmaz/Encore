#!/usr/bin/env bash
# A made-up music library for the performance run (PERFORMANCE.md, "In CI"): ALBUMS albums of TRACKS songs each,
# 4 albums per artist, every album with its own 1200×1200 cover.jpg (so covers are decoded and scaled as with real
# ones). The songs are one 45 s FLAC tone, copied with each song's own tags (title, artist, album, track number).
# Usage: make-library.sh <dir> [albums=80] [tracks=10]
set -euo pipefail
DIR=${1:?usage: make-library.sh <dir> [albums] [tracks]}
ALBUMS=${2:-80}
TRACKS=${3:-10}
mkdir -p "$DIR"
TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT
# one tone for every song: tags differ, the audio doesn't need to
ffmpeg -loglevel error -y -f lavfi -i "sine=frequency=440:sample_rate=44100:duration=45" -c:a flac "$TMP/tone.flac"
for a in $(seq 1 "$ALBUMS"); do
  artist="Bench Artist $(( (a - 1) / 4 + 1 ))"
  album=$(printf "Bench Album %03d" "$a")
  d="$DIR/$artist/$album"
  mkdir -p "$d"
  # a different frame of the test pattern per album: covers look different and can't be de-duplicated
  ffmpeg -loglevel error -y -f lavfi -i "testsrc2=size=1200x1200:rate=1" -ss "$a" -frames:v 1 -q:v 3 "$d/cover.jpg"
  for t in $(seq 1 "$TRACKS"); do
    ffmpeg -loglevel error -y -i "$TMP/tone.flac" -c copy \
      -metadata title="Song $t of $album" -metadata artist="$artist" -metadata album_artist="$artist" \
      -metadata album="$album" -metadata track="$t/$TRACKS" -metadata date="$(( 1980 + a % 40 ))" -metadata genre="Electronic" \
      "$d/$(printf '%02d' "$t") Song $t.flac"
  done
done
echo "made $ALBUMS albums × $TRACKS songs in $DIR"
