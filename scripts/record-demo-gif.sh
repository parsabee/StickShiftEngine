#!/usr/bin/env bash
# Record the playable demo and write docs/images/demo.gif, the README's GIF.
#
# Runs the real game offscreen through the Metal backend with a scripted autopilot
# (games/playable-demo/examples/record_demo.rs), so no window or screen capture is
# involved, then assembles the frames with macOS's ImageIO (scripts/frames-to-gif.swift).
# Traffic is seeded, so a run records the same clip. macOS only.
#
#   ./scripts/record-demo-gif.sh [out.gif]
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
cd "$root"
out="${1:-docs/images/demo.gif}"
frames="target/record-demo"

rm -rf "$frames"
cargo run --quiet --release -p playable-demo --example record_demo -- "$frames"
# 600 px wide at 20 frames per second (the recorder keeps every third 60 Hz frame).
swift scripts/frames-to-gif.swift "$out" 600 0.05 "$frames"/frame_*.png
echo "wrote $out ($(stat -f %z "$out") bytes)"
