import type { Finding, InventorySnapshot } from './inventory.ts';

const healthGroups = [
  ['machines', 'machine-health'],
  ['services', 'service-health'],
  ['backups', 'backup-health'],
] as const;

export const findingGroups = [
  ['guest-mismatch', 'Confirmed guest mismatches'],
  ['record-review', 'Deployment records to review'],
  ['machine-health', 'Machine health'],
  ['service-health', 'Service health'],
  ['backup-health', 'Backup jobs'],
  ['network', 'IP reservation audit'],
  ['lease', 'Environment leases'],
  ['warning', 'Warnings'],
  ['coverage', 'Checks we could not verify'],
] as const;

export const tally = (items: Finding[]) => items.reduce((sum, item) => sum + (item.count ?? 1), 0);

export function age(stamp: number, now: number): string {
  const minutes = Math.max(0, Math.floor((now - stamp) / 60));
  if (minutes < 1) return 'Just now';
  if (minutes < 60) return minutes + 'm ago';
  if (minutes < 1440) return Math.floor(minutes / 60) + 'h ago';
  return Math.floor(minutes / 1440) + 'd ago';
}

export function describeDiscrepancies(inventory: InventorySnapshot, now: number) {
  const findings: Finding[] = [];
  for (const [collection, category] of healthGroups) {
    for (const record of inventory[collection]) {
      if (record.lifecycle !== 'active') continue;
      if (category === 'machine-health' && record.runtime?.state === 'stopped' &&
          (record.runtime.expires ?? 0) >= now) continue;
      const target = record.display_name || record.name;
      const state = record.health;
      if (!inventory.error && state?.status === 'unhealthy' && (state.expires ?? 0) > now) {
        findings.push({ category, target, message: state.evidence || 'Health check failed.' });
      } else if (state?.status === 'stale' || (state?.expires && state.expires <= now)) {
        findings.push({ category: 'coverage', target, message: 'The last health check expired.' });
      } else if ((state?.status === 'unknown' || !state?.status) && (state?.checked || category === 'backup-health')) {
        findings.push({
          category: 'coverage', target,
          message: category === 'backup-health' ? 'No verified backup result.' : 'Health check inconclusive.',
        });
      }
    }
  }

  const leaseReport = inventory.lease_issues;
  if (leaseReport?.status === 'verified') {
    const messages = {
      'export-failed': 'Workspace export failed. See Leases for details.',
      'overdue': 'The lease is overdue.',
    };
    for (const issue of leaseReport.issues) {
      findings.push({ category: 'lease', target: issue.task, message: messages[issue.code] });
    }
  } else if (leaseReport) {
    findings.push({
      category: 'coverage', target: 'Leases',
      message: leaseReport.status === 'checking' ? 'Checking current leases.' : 'Could not check current leases.',
    });
  }

  const audit = inventory.address_audit;
  if (audit) {
    const target = 'IP reservation audit';
    const date = new Date(audit.at).toLocaleString();
    const count = audit.unreserved_static ?? 0;
    if (audit.status === 'error' || audit.status === 'failed') {
      findings.push({ category: 'network', target, message: 'The audit failed on ' + date + '.' });
    } else if (count > 0) {
      findings.push({
        category: 'network', target, count,
        message: count + ' unreserved static ' + (count === 1 ? 'address' : 'addresses') + ' in the audit from ' + date + '.',
      });
    }
    if (['incomplete', 'partial', 'unknown', 'unverified'].includes(audit.status)) {
      findings.push({ category: 'coverage', target, message: 'Audit coverage is ' + audit.status + ' as of ' + date + '.' });
    }
    if (audit.warnings > 0) {
      findings.push({
        category: 'warning', target, count: audit.warnings,
        message: audit.warnings + ' ' + (audit.warnings === 1 ? 'warning' : 'warnings') + ' in the audit from ' + date + '.',
      });
    }
  } else if (inventory.fetched_at) {
    findings.push({ category: 'coverage', target: 'IP reservation audit', message: 'No audit result has been published.' });
  }

  const catalog = inventory.catalog;
  if (inventory.error && inventory.fetched_at) {
    findings.push({ category: 'coverage', target: 'Registry', message: inventory.error });
  }
  if (catalog && catalog.feed !== 'connected') {
    const read = catalog.checked ? age(catalog.checked, now).toLowerCase() : 'at startup';
    findings.push({ category: 'coverage', target: 'Registry catalog',
      message: 'Live catalog updates unavailable. Cached records were last read ' + read + '.' });
  }

  const accuracy = inventory.inventory_accuracy;
  const stale = !accuracy || !!inventory.error || now - accuracy.checked > 90;
  const catalogUnverified = catalog && catalog.feed !== 'connected' && (!catalog.checked || now - catalog.checked > 90);
  const expiredHosts = new Set(accuracy?.hosts
    .filter(host => host.status === 'verified' && (host.expires ?? Infinity) <= now)
    .map(host => host.machine_id));
  if (accuracy) {
    const expiredDeployments = new Set(accuracy.deployments
      ?.filter(entry => entry.state === 'missing' && (inventory.error || entry.expires <= now))
      .map(entry => entry.record_id));
    for (const finding of accuracy.findings) {
      if (finding.category === 'guest-mismatch' &&
          (stale || catalogUnverified || expiredHosts.has(finding.machine_id || finding.target))) continue;
      if (finding.code === 'deployment-missing' && expiredDeployments.has(finding.target)) continue;
      findings.push(finding);
    }
    for (const target of expiredDeployments) {
      findings.push({ category: 'coverage', target, message: 'The deployment state could not be verified.' });
    }
  }

  const unverified = tally(findings.filter(item => item.category === 'coverage'));
  const warnings = tally(findings.filter(item => item.category === 'warning'));
  const problems = tally(findings) - unverified - warnings;
  const summary = [!inventory.fetched_at ? 'Waiting for lab reports…' : problems > 0
    ? problems + ' ' + (problems === 1 ? 'problem' : 'problems') : 'No confirmed problems'];
  if (warnings) summary.push(warnings + ' ' + (warnings === 1 ? 'warning' : 'warnings'));
  if (unverified) summary.push(unverified + ' unverified ' + (unverified === 1 ? 'check' : 'checks'));

  const coverage = accuracy?.coverage;
  const scope = [coverage
    ? (coverage.verified - expiredHosts.size) + ' of ' + coverage.expected + ' configured hosts verified'
    : 'Guest inventory unavailable'];
  if (stale && coverage) scope.push('Guest inventory needs a refresh');
  if (coverage?.unconfigured) {
    const noun = coverage.unconfigured === 1 ? ' host has' : ' hosts have';
    scope.push(coverage.unconfigured + noun + ' no configured guest collector');
  }
  if (coverage?.not_applicable) {
    const noun = coverage.not_applicable === 1 ? ' machine' : ' machines';
    scope.push(coverage.not_applicable + noun + ' outside guest inventory scope');
  }
  if (audit) scope.push('Last IP audit ' + audit.status + ' · ' + new Date(audit.at).toLocaleString());
  return { problems, summary: summary.join(' · '), scope: scope.join(' · '), findings };
}
