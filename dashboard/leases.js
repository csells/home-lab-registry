'use strict';

const LeaseHistory = (() => {
  const cards = document.querySelector('#leases');
  const older = document.querySelector('#older');
  let loading = false;
  const message = document.querySelector('#lease-error');
  let cursor = null;
  function node(tag, text) {
    const element = document.createElement(tag);
    element.textContent = text;
    return element;
  }
  function date(value) {
    return new Date(value * 1000).toLocaleString();
  }
  function card(lease) {
    const section = document.createElement('article');
    section.className = 'lease';
    const header = document.createElement('header');
    header.append(node('h3', lease.task), node('strong', lease.phase.replaceAll('-', ' ')));
    const fields = document.createElement('dl');
    function field(label, value) { fields.append(node('dt', label), node('dd', value)); }
    field('Owner', lease.owner);
    if (lease.released_at) field('Released', date(lease.released_at));
    else field('Renew by', date(lease.deadline));
    if (lease.hold_mode) field('Review hold', lease.hold_mode + ' · ' + lease.hold_reason);
    if (lease.reviewer) field('Reviewer', lease.reviewer);
    if (lease.reviewer_hold) field('Reviewer hold', lease.reviewer_hold + ' · ' + lease.reviewer_reason);
    if (lease.export_error) field('Export failure', lease.export_error);
    field('Recovery points', String(lease.checkpoint_count));
    if (lease.last_verified_at) field('Last verified', date(lease.last_verified_at));
    if (lease.pinned) field('Latest checkpoint', 'Pinned');
    else if (lease.retained_through) field('Retained until', date(lease.retained_through));
    section.append(header, fields);
    return section;
  }
  async function load(append = false) {
    if (loading) return;
    loading = older.disabled = true;
    message.hidden = true;
    try {
      const response = await fetch('/api/leases' + (append ? '?before=' + cursor : ''), {cache:'no-store', signal:AbortSignal.timeout(12000)});
      if (!response.ok) throw new Error();
      const page = await response.json();
      if (!append) cards.replaceChildren();
      cards.append(...page.leases.map(card));
      if (!cards.childElementCount) cards.append(node('p', 'No leases recorded.'));
      cursor = page.next_before;
      older.hidden = cursor === null;
      document.querySelector('#lease-as-of').textContent = 'Updated ' + date(page.as_of);
    } catch {
      message.textContent = 'Lease history could not be refreshed. Try again.';
      message.hidden = false;
    } finally {
      loading = older.disabled = false;
    }
  }
  older.onclick = () => load(true);
  return {load};
})();
