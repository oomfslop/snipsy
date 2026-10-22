#!/bin/sh
set -eu

cd "$(dirname "$0")/.."
profile="${1:-release}"
if [ "$profile" = release ]; then
    cargo build --release
else
    cargo build
fi
bundle="$(python3 scripts/bundle_macos.py "$profile" | tail -n 1)"

installed=/Applications/Snipsy.app/Contents/MacOS/snipsy
if pgrep -f "$installed" >/dev/null; then
    osascript -e 'quit app id "desu.tei.snipsy"'
    while pgrep -f "$installed" >/dev/null; do sleep 0.2; done
fi

rm -rf /Applications/Snipsy.app
ditto "$bundle" /Applications/Snipsy.app
open /Applications/Snipsy.app
