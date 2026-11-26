#!/usr/bin/env python3
"""Print the changelog section for a version tag.

The release workflow runs this for the pushed tag and posts the result as the
release body, so the GitHub release and `CHANGELOG.md` cannot drift apart.

    scripts/changelog_section.py v5.9.0 > release-notes.md

Exits non-zero when the section is missing: a release without notes is a
mistake worth failing the build over, not something to paper over with
generated commit titles.
"""

import pathlib
import re
import sys

if len(sys.argv) != 2:
    raise SystemExit("usage: changelog_section.py <tag>")

tag = sys.argv[1]
version = tag[1:] if tag.startswith("v") else tag

text = pathlib.Path("CHANGELOG.md").read_text(encoding="utf-8")

# A section starts at "## 5.9.0" and ends at the next "## " heading.
heading = re.compile(rf"^## {re.escape(version)}(?:\s|$)")
lines = text.splitlines()

start = None
for index, line in enumerate(lines):
    if heading.match(line):
        start = index
        break

if start is None:
    raise SystemExit(f"CHANGELOG.md has no section for {version}")

end = len(lines)
for index in range(start + 1, len(lines)):
    if lines[index].startswith("## "):
        end = index
        break

section = "\n".join(lines[start:end]).rstrip() + "\n"
sys.stdout.write(section)
