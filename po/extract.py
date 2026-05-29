#!/usr/bin/env python3
"""Extract translatable strings from t!()/tn!() macro calls into po/leyen.pot.

Rust-aware (xgettext cannot parse the `name!(...)` macro form). Scans for:
    t!("msgid")
    tn!("singular", "plural", ...)
Collects unique entries with source references and writes a gettext .pot.
"""
import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
SRC = ROOT / "src"

STR = r'"((?:[^"\\]|\\.)*)"'
RE_T = re.compile(r'\bt!\(\s*' + STR + r'\s*\)')
RE_TN = re.compile(r'\btn!\(\s*' + STR + r'\s*,\s*' + STR + r'\s*,')


def main() -> int:
    # msgid -> {"refs": [...], "plural": Optional[str]}
    entries: dict[str, dict] = {}

    def add(msgid: str, ref: str, plural: str | None = None):
        e = entries.setdefault(msgid, {"refs": [], "plural": None})
        if ref not in e["refs"]:
            e["refs"].append(ref)
        if plural:
            e["plural"] = plural

    for path in sorted(SRC.rglob("*.rs")):
        rel = path.relative_to(ROOT).as_posix()
        for lineno, line in enumerate(path.read_text(encoding="utf-8").splitlines(), 1):
            for m in RE_T.finditer(line):
                add(m.group(1), f"{rel}:{lineno}")
            for m in RE_TN.finditer(line):
                add(m.group(1), f"{rel}:{lineno}", m.group(2))

    out = [
        'msgid ""',
        'msgstr ""',
        '"Project-Id-Version: leyen\\n"',
        '"Report-Msgid-Bugs-To: \\n"',
        '"Language: \\n"',
        '"MIME-Version: 1.0\\n"',
        '"Content-Type: text/plain; charset=UTF-8\\n"',
        '"Content-Transfer-Encoding: 8bit\\n"',
        '"Plural-Forms: nplurals=2; plural=(n != 1);\\n"',
        "",
    ]

    for msgid in sorted(entries):
        e = entries[msgid]
        for ref in e["refs"]:
            out.append(f"#: {ref}")
        if e["plural"] is not None:
            out.append(f'msgid "{msgid}"')
            out.append(f'msgid_plural "{e["plural"]}"')
            out.append('msgstr[0] ""')
            out.append('msgstr[1] ""')
        else:
            out.append(f'msgid "{msgid}"')
            out.append('msgstr ""')
        out.append("")

    (ROOT / "po" / "leyen.pot").write_text("\n".join(out), encoding="utf-8")
    n_plural = sum(1 for e in entries.values() if e["plural"])
    print(f"extracted {len(entries)} messages ({n_plural} plural) -> po/leyen.pot")
    return 0


if __name__ == "__main__":
    sys.exit(main())
