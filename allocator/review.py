"""Read-only projections of lease history for authenticated lab operators."""
import json
import time

from checkpoints import Rejected


def lease_page(db, before, scope='all'):
    predicates = {'all': '1', 'current': "phase != 'released'", 'released': "phase = 'released'"}
    if scope not in (*predicates, 'counts'):
        raise Rejected('invalid_scope', 'Unknown lease scope.', 400)
    if before is not None and (not before.isascii() or not before.isdigit()
                               or len(before) > 18 or int(before) < 1):
        raise Rejected('invalid_cursor', 'Invalid lease history cursor.', 400)
    now = time.time()
    current, released = db.execute("""SELECT COUNT(*) FILTER (WHERE phase != 'released'),
                                            COUNT(*) FILTER (WHERE phase = 'released') FROM lease""").fetchone()
    summary = {'counts': {'current': current, 'released': released}, 'as_of': now}
    if scope == 'counts':
        if before is not None:
            raise Rejected('invalid_cursor', 'Counts cannot be paginated.', 400)
        return summary
    rows = db.execute(f"""SELECT rowid AS cursor, id, task, owner, phase, deadline,
                            released_at, hold_mode, hold_reason, reviewer, reviewer_hold, reviewer_reason, export_error, checkpoint
                         FROM lease WHERE {predicates[scope]} AND (? IS NULL OR rowid < ?)
                         ORDER BY rowid DESC LIMIT 26""", (before, before)).fetchall()
    result = []
    for row in rows[:25]:
        item = {key: row[key] for key in ('id', 'task', 'owner', 'phase', 'deadline',
                'released_at', 'hold_mode', 'hold_reason', 'reviewer', 'reviewer_hold', 'reviewer_reason', 'export_error')}
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
    return {**summary, 'leases': result, 'next_before': rows[24]['cursor'] if len(rows) > 25 else None}
