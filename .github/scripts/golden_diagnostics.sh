#!/usr/bin/env bash
# Shared diagnostic commands; callers retain separate two/five-minute step deadlines.
set -euo pipefail
out=target/golden/crashes
mkdir -p "$out"
bin=target/debug
content="$PWD/target/content/desktop"
if [ "${GOLDEN_PREBUILT:-false}" = true ]; then
  bin=target/golden-input/bin
  content="$PWD/target/golden-input/content"
fi
case "$1" in
  gather)
    case "$RUNNER_OS" in
      Windows) cache="$(cygpath -u "$LOCALAPPDATA")" ;;
      macOS) cache="$HOME/Library/Caches" ;;
      *) cache="${XDG_CACHE_HOME:-$HOME/.cache}" ;;
    esac
    cp -R "$cache/Sapphire/Sapphire Sandbox/crashes/." "$out/" 2>/dev/null || echo "no crash reports"
    cp "$bin/sapphire_sandbox.pdb" "$out/" 2>/dev/null || true
    ;;
  replay)
    debug=validation
    # Metal validation is broken on the hosted paravirtual device.
    case "$RUNNER_OS" in
      Windows) content="$(cygpath -w "$content")" ;;
      macOS) debug=none ;;
    esac
    SAPPHIRE_GPU_DEBUG=$debug SAPPHIRE_CONTENT_DIR="$content" "$bin/sapphire_sandbox" \
      --capture "$out/serial-passes.png" --size 640x360 --capture-ticks 0 --plain-post --plain-sky \
      --aa msaa --serial-passes > "$out/serial-passes.log" 2>&1 \
      || echo "the serial-passes run failed with exit code $?"
    tail -n 30 "$out/serial-passes.log"
    ;;
  *) echo "unknown golden diagnostic: $1" >&2; exit 2 ;;
esac
