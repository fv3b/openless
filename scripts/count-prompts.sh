#!/bin/bash
set -uo pipefail

OUT="${1:-/tmp/openless-dialogs.log}"
shift
FRAMES_DIR="$(mktemp -d /tmp/openless-frames.XXXXXX)"
rm -f "$OUT"
echo "frames_dir=$FRAMES_DIR" > "$OUT"

(
  T0=$(date +%s)
  while true; do
    NOW=$(date +%s)
    ELAPSED=$((NOW - T0))
    TS=$(date +%H:%M:%S)
    N=$(printf "%03d" ${#ELAPSED})
    screencapture -x "$FRAMES_DIR/f-${ELAPSED}.png" 2>/dev/null
    if pgrep -x SecurityAgent >/dev/null 2>&1; then
      echo "$ELAPSED SecurityAgent-present" >> "$OUT"
    fi
    if [ "$ELAPSED" -ge 300 ]; then
      break
    fi
    sleep 3
  done
) &
MON_PID=$!

"$@"
STATUS=$?

sleep 120
kill $MON_PID 2>/dev/null || true

PROMPTS=$(grep -c "SecurityAgent-present" "$OUT" 2>/dev/null || echo 0)
echo "dialogs=$PROMPTS command-exit=$STATUS frames=$FRAMES_DIR"
[ "$PROMPTS" = "0" ] || grep "SecurityAgent-present" "$OUT"
