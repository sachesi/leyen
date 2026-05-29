#!/usr/bin/env bash
# Regenerate the template and compile every translation catalog into
# packaging/usr/share/locale/<lang>/LC_MESSAGES/leyen.mo
set -euo pipefail

cd "$(dirname "$0")/.."

python3 po/extract.py

for po in po/*.po; do
    lang=$(basename "$po" .po)
    # Merge new template strings into the existing translation (keeps msgstrs).
    msgmerge --update --backup=none --quiet "$po" po/leyen.pot
    dest="packaging/usr/share/locale/$lang/LC_MESSAGES"
    mkdir -p "$dest"
    msgfmt --check "$po" -o "$dest/leyen.mo"
    echo "compiled $po -> $dest/leyen.mo"
done
