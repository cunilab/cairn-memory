#!/usr/bin/env python3
"""Provision the pinned MiniLM artifact; the server never downloads a model."""
import hashlib
import json
import pathlib
import sys
import urllib.request


def download(directory):
    manifest = json.loads((pathlib.Path(__file__).resolve().parent.parent / "models/minilm.json").read_text())
    directory.mkdir(parents=True, exist_ok=True)
    for name, digest in manifest["files"].items():
        target = directory / name
        if target.is_file() and hashlib.sha256(target.read_bytes()).hexdigest() == digest:
            continue
        temporary = target.with_suffix(target.suffix + ".download")
        try:
            url = f'https://huggingface.co/{manifest["repository"]}/resolve/{manifest["revision"]}/{name}'
            with urllib.request.urlopen(url, timeout=120) as source, temporary.open("wb") as output:
                check = hashlib.sha256()
                while block := source.read(1024 * 1024):
                    output.write(block)
                    check.update(block)
            if check.hexdigest() != digest:
                raise ValueError(f"checksum mismatch: {name}")
            temporary.replace(target)
        finally:
            temporary.unlink(missing_ok=True)
    print(f"Pinned MiniLM model verified in {directory}")


if __name__ == "__main__":
    if len(sys.argv) != 2:
        raise SystemExit("usage: download_embedding_model.py <model-directory>")
    download(pathlib.Path(sys.argv[1]))
