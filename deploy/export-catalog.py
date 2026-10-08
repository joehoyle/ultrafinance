#!/usr/bin/env python3
"""Export merchant records from an existing SQLite DB for an image build."""
import json
import sqlite3
import sys
from pathlib import Path

if len(sys.argv) != 3:
    sys.exit("usage: export-catalog.py DATABASE OUTPUT.json")
database = Path(sys.argv[1]).resolve()
# mode=ro prevents a typo from creating an empty database.
with sqlite3.connect(database.as_uri() + "?mode=ro", uri=True) as connection:
    records = [json.loads(row[0]) for row in connection.execute("SELECT data FROM merchants ORDER BY id")]
Path(sys.argv[2]).write_text(json.dumps(records, indent=2) + "\n")
print(f"Exported {len(records)} merchants")
