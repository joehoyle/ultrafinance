"""Numeric-only ECS progress. Never print URLs, object names, SQL or diagnostics."""
import pathlib
import re
import sys
import time


def download_position(manifest, directory, verified):
    total = position = 0
    for line in manifest.read_text().splitlines():
        name, _checksum, size, _url = line.split("\t", 3)
        size = int(size)
        total += size
        try:
            current = (directory / name).stat().st_size
        except FileNotFoundError:
            current = 0
        # A downloaded file must also pass its checksum before counting in full.
        limit = size if (verified / name).exists() else max(0, size - 1)
        position += min(current, limit)
    return position, total


def restore_position(log, toc):
    total = sum(bool(line.strip()) and not line.startswith(";") for line in toc.read_text().splitlines())
    try:
        text = log.read_text(errors="replace")
    except FileNotFoundError:
        text = ""
    completed = set(re.findall(r"^pg_restore: finished item (\d+) ", text, re.MULTILINE))
    # Count confirmed completed TOC objects. Metadata work performed serially is
    # accounted for after pg_restore exits successfully. Objects vary in size.
    return min(len(completed), max(0, total - 1)), total


def main():
    stage = sys.argv[1]
    previous = 0
    while True:
        if stage == "download":
            done, total = download_position(pathlib.Path("downloads"), pathlib.Path("dump"), pathlib.Path("verified"))
            unit = "bytes"
        elif stage == "restore":
            done, total = restore_position(pathlib.Path("restore.log"), pathlib.Path("toc.list"))
            unit = "items"
        else:
            raise ValueError("unsupported progress stage")
        previous = max(previous, done)
        print(f"UF_UPLOAD_PROGRESS:{stage}:{previous}:{total}:{unit}", flush=True)
        time.sleep(1)


if __name__ == "__main__":
    main()
