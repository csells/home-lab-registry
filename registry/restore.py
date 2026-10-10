#!/usr/bin/env python3
"""Restore an exported registry into a NEW database with its complete revision history."""
import json, pathlib, sqlite3, sys
from registry import Catalog
path=pathlib.Path(sys.argv[2])
if path.exists(): raise SystemExit('Destination already exists; restore to a new database')
export=json.loads(pathlib.Path(sys.argv[1]).read_text()); catalog=Catalog(str(path))
with catalog.db() as db:
    for r in export['services']:
        db.execute('INSERT INTO services VALUES(?,?,?,?)',(r['id'],r['revision'],json.dumps(r['spec']),r['updated_at']))
        for h in export['history'][r['id']]:
            db.execute('INSERT INTO history(id,revision,document,actor,updated) VALUES(?,?,?,?,?)',(r['id'],h['revision'],h['document'],h['actor'],h['updated']))
# Observations intentionally reset to unknown; they must be freshly checked.
path.chmod(0o600)
print('Restored',len(export['services']),'services; health will be rechecked')
