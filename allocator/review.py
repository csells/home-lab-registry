"""Read-only projections of lease history for authenticated lab operators."""
import json
import time

from checkpoints import Rejected


def lease_page(db, before):
    if before is not None and (not before.isascii() or not before.isdigit()
                               or len(before) > 18 or int(before) < 1):
        raise Rejected('invalid_cursor', 'Invalid lease history cursor.', 400)
    rows = db.execute("""SELECT rowid AS cursor, id, task, owner, phase, deadline,
                            released_at, hold_mode, hold_reason, export_error, checkpoint
                         FROM lease WHERE (? IS NULL OR rowid < ?)
                         ORDER BY rowid DESC LIMIT 26""", (before, before)).fetchall()
    now = time.time()
    result = []
    for row in rows[:25]:
        item = {key: row[key] for key in ('id', 'task', 'owner', 'phase', 'deadline',
                'released_at', 'hold_mode', 'hold_reason', 'export_error')}
        if item['phase'] == 'ready' and item['deadline'] <= now:
            item['phase'] = 'overdue'
        saved = db.execute("""SELECT receipt FROM incremental_checkpoint
                              WHERE lease_id=? AND receipt IS NOT NULL""", (row['id'],))
        receipts = [json.loads(entry['receipt']) for entry in saved]
        if row['checkpoint']:
            receipts.append(json.loads(row['checkpoint']))
        accessible = [r for r in receipts if not r.get('purged_at') and not r.get('purge_claim')
                      and (r.get('pin') or r.get('retain_through', now + 1) > now)]
        last = max(accessible, key=lambda r: r['verified_at'], default={})
        item.update(checkpoint_count=len(accessible), last_verified_at=last.get('verified_at'),
                    retained_through=last.get('retain_through'), pinned=bool(last.get('pin')))
        result.append(item)
    return {'leases': result, 'as_of': now, 'next_before': rows[24]['cursor'] if len(rows) > 25 else None}
