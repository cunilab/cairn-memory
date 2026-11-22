#!/usr/bin/env python3
"""A 0.1.5 `cairn update` downloads cairn-<target>, SHA256SUMS and three hook assets from the
release it installs (v0.1.5 crates/client/src/update.rs HOOK_ASSETS/TARGETS) and aborts when one
is missing. Check that release.yml still publishes every one of them."""

import re
import subprocess
import sys
from pathlib import Path

root = Path(__file__).resolve().parent.parent
workflow = (root / ".github/workflows/release.yml").read_text()
stubs = root / "release/legacy-update-stubs"

# What the installed 0.1.5 client asks for.
legacy_targets = ["aarch64-apple-darwin", "x86_64-unknown-linux-musl",
                  "aarch64-unknown-linux-musl", "x86_64-pc-windows-msvc"]
legacy_hooks = ["cairn_hook.py", "cairn_outbox.py", "cairn_instructions.txt"]
needed = {"SHA256SUMS", *legacy_hooks}
needed |= {f"cairn-{t}{'.exe' if 'windows' in t else ''}" for t in legacy_targets}

# What release.yml publishes: build matrix binaries, stubs copied into dist/, SHA256SUMS over dist/*.
matrix = re.findall(r'target: (\S+?), server_target: \S+?, ext: "(\.exe)?"', workflow)
published = {f"cairn-{t}{ext}" for t, ext in matrix} | {"SHA256SUMS"}
published |= {p.name for p in stubs.iterdir()}
if "release/legacy-update-stubs/*" not in workflow:
    published -= {p.name for p in stubs.iterdir()}  # stubs exist but the workflow never ships them
if not re.search(r"sha256sum -- \*", workflow):
    published.discard("SHA256SUMS")

errors = [f"release.yml does not publish {name}" for name in sorted(needed - published)]
for name in legacy_hooks:
    if not (stubs / name).is_file() or not (stubs / name).read_text().strip():
        errors.append(f"stub {name} is missing or empty")
for script in legacy_hooks[:2]:
    run = subprocess.run([sys.executable, str(stubs / script)], input="{}", text=True, capture_output=True)
    if run.returncode != 0:
        errors.append(f"stub {script} exits {run.returncode}; an old hook entry must not fail")
if errors:
    sys.exit("\n".join(errors))
print(f"ok: release.yml publishes all {len(needed)} assets a 0.1.5 `cairn update` needs")
