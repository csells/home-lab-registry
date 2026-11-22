"""Read-only projections of lease history for authenticated lab operators."""
import json
import time

from checkpoints import Rejected, recoverable, restore_held


def lease_inventory(db, scopes):
    """Project exact live clone intents without provider calls or task secrets."""
    rows = db.execute("SELECT id, slot_id FROM lease WHERE phase != 'released'")
    guests = []
    for row in rows:
        scope = scopes.get(row['slot_id'])
        if scope is not None:
            guests.append({'node': scope.node, 'id': scope.guest_id,
                           'name': scope.name(row['id']), 'type': 'qemu', 'lease': row['id']})
    return {'guests': guests}


def lease_issues(db, now):
    rows = db.execute("""SELECT task, deadline, phase, export_error, hold_mode, reviewer_hold
                         FROM lease WHERE phase IN ('ready', 'awaiting-export') OR
                         (phase != 'released' AND export_error IS NOT NULL) ORDER BY rowid""")
    issues = []
    next_deadline = None
    for row in rows:
        if row['export_error']:
            issues.append({'task':row['task'],'code':'export-failed'})
        elif row['phase'] == 'ready' and not (row['hold_mode'] or row['reviewer_hold']):
            deadline = row['deadline']
            if deadline <= now:
                issues.append({'task':row['task'],'code':'overdue'})
            elif next_deadline is None or deadline < next_deadline:
                next_deadline = deadline
    return {'issues':issues,'next_deadline':next_deadline}


def lease_page(db, before, scope='all'):
    predicates = {'all': '1', 'current': "phase != 'released'", 'released': "phase = 'released'"}
    if scope not in (*predicates, 'counts', 'issues'):
        raise Rejected('invalid_scope', 'Unknown lease scope.', 400)
    if before is not None and (not before.isascii() or not before.isdigit()
                               or len(before) > 18 or int(before) < 1):
        raise Rejected('invalid_cursor', 'Invalid lease history cursor.', 400)
    now = time.time()
    current, released = db.execute("""SELECT COUNT(*) FILTER (WHERE phase != 'released'),
                                            COUNT(*) FILTER (WHERE phase = 'released') FROM lease""").fetchone()
    summary = {'counts': {'current': current, 'released': released}, 'as_of': now}
    if scope in ('counts', 'issues'):
        if before is not None:
            raise Rejected('invalid_cursor', 'Summary cannot be paginated.', 400)
        if scope == 'issues':
            summary.update(lease_issues(db, now))
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
        saved = db.execute("""SELECT id, receipt FROM incremental_checkpoint
                              WHERE lease_id=? AND receipt IS NOT NULL""", (row['id'],))
        receipts = [(entry['id'], json.loads(entry['receipt'])) for entry in saved]
        if row['checkpoint']:
            receipts.append((None, json.loads(row['checkpoint'])))
        accessible = [r for key, r in receipts if recoverable(
            r, row['phase'], key is None, now, restore_held(db, row['id'], key))]
        last = max(accessible, key=lambda r: r['verified_at'], default={})
        item.update(checkpoint_count=len(accessible), last_verified_at=last.get('verified_at'),
                    retained_through=last.get('retain_through'), pinned=bool(last.get('pin')))
        result.append(item)
    return {**summary, 'leases': result, 'next_before': rows[24]['cursor'] if len(rows) > 25 else None}
