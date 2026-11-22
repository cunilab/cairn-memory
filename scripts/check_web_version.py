#!/usr/bin/env python3
"""The web console's package version must equal the workspace version (one number per release).
Exit 1 and say which file differs; releases bump `[workspace.package]` and these two files."""

import json
import re
import sys
from pathlib import Path

root = Path(__file__).resolve().parent.parent
workspace = re.search(
    r'\[workspace\.package\][^\[]*?^version\s*=\s*"([^"]+)"', (root / "Cargo.toml").read_text(), re.M | re.S
).group(1)
package = json.loads((root / "apps/web/package.json").read_text())["version"]
lock = json.loads((root / "apps/web/package-lock.json").read_text())
found = {
    "apps/web/package.json": package,
    "apps/web/package-lock.json": lock["version"],
    'apps/web/package-lock.json packages[""]': lock["packages"][""]["version"],
}
bad = {name: version for name, version in found.items() if version != workspace}
for name, version in bad.items():
    print(f"{name} is {version}, the workspace version is {workspace}", file=sys.stderr)
sys.exit(1 if bad else 0)
