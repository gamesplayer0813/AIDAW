#!/usr/bin/env bash
# Put together the Windows download: AIDAW/ with both programs, the docs a
# user needs, the demo song and the licences, and AIDAW-win64.zip of it.
#
#   packaging/package-windows.sh BIN_DIR OUT_DIR
#
# BIN_DIR holds aidaw.exe and aidaw-mcp.exe (target/release after
# `cargo build --release`). Used by CI and the release workflow; runs in Git
# Bash on Windows and in any Linux shell.
set -euo pipefail

if [[ $# -ne 2 ]]; then
    echo "usage: $0 BIN_DIR OUT_DIR" >&2
    exit 2
fi
bin=$1
out=$2
root=$(cd "$(dirname "$0")/.." && pwd)
stage=$out/AIDAW
zip=AIDAW-win64.zip

rm -rf "$stage" "${out:?}/$zip"
mkdir -p "$stage"
cp "$bin/aidaw.exe" "$bin/aidaw-mcp.exe" "$stage/"
cp "$root/packaging/README.txt" "$root/docs/LLM_GUIDE.md" "$root/docs/TOOLS.md" \
    "$root/examples/demo.aidaw.json" "$stage/"
cp "$root/LICENSE" "$stage/LICENSE.txt"
cp "$root/crates/aidaw-gui/assets/Inter-LICENSE.txt" \
    "$root/crates/aidaw-gui/assets/NotoSansSC-LICENSE.txt" "$stage/"

# The zip holds the AIDAW folder, so unpacking it never scatters files.
if command -v 7z >/dev/null; then
    (cd "$out" && 7z a -tzip -bso0 -bsp0 "$zip" AIDAW)
elif command -v zip >/dev/null; then
    (cd "$out" && zip -qr "$zip" AIDAW)
else
    (cd "$out" && powershell -NoProfile -Command "Compress-Archive -Path AIDAW -DestinationPath $zip")
fi
echo "packaged $out/$zip:"
ls -l "$stage"
