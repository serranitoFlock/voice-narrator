#!/usr/bin/env bash
# Dumps the bash pipeline's text preparation for every finalized marker:
# one JSON line {text, clean, sentences} per marker.
source "$HOME/voice-pipeline/scripts/tts-common.sh"
LOG_FILE=/dev/null
for f in "$HOME"/voice-pipeline/tmp/dispatch/*.txt.finalized; do
  text="$(jq -r '.text // empty' "$f")"
  [ -z "$text" ] && continue
  clean="$(clean_for_tts "$text")"
  cut_text="$(printf '%s' "$clean" | cut -c1-1500)"
  sentences="$(printf '%s' "$cut_text" | perl -pe 's/(?<=[.!?])\s+/\x00/g' | jq -Rs 'split("\u0000")')"
  jq -cn --arg text "$text" --arg clean "$clean" --argjson sentences "$sentences" '{text: $text, clean: $clean, sentences: $sentences}'
done
