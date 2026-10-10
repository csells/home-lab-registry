# Central machine and service registry

The registry is the sole source of information about available machines and their setup, as well as service deployments. Keep machine identity, hardware, installed software, templates, addresses, access references, recovery instructions, reservations and validation evidence in records, never in skill files. This document defines discovery and update procedures only.

## Bootstrap and discovery

Resolve the registry URL, TLS CA and credentials from private `~/.config/home-lab/registry.json`, `LAB_REGISTRY_CONFIG`, or the client's `--config PATH`. The private config contains `url`, `ca_file`, `reader_token` and optionally `writer_token`. Protect it with mode 0600. Give consumers reader access only; never print tokens or put secrets in catalog records, command arguments or Git. Do not hardcode a registry host/address in this skill.

Resolve the installed skill directory and invoke its `scripts/registry.py` client. Examples below use `registry.py` as shorthand for that resolved command:

```sh
registry.py list --capability machine
registry.py get RECORD_ID
registry.py list --capability REQUIRED_CAPABILITY
registry.py list --host HOST_NAME
```

Machine records use the `machine` capability and `spec.resources.kind: "machine"`; IDs follow `machine-<name>`. Services and machines share the existing catalog API namespace. Discover IDs through the list response rather than assuming names. Historical evidence records are archives, not available workers.

Read `spec.resources`, `runbook`, `credential_ref`, `notes`, dependencies, endpoints and lifecycle. Runbooks may be inline under `resources.runbooks`; follow the record's pointer. Machine records describe hardware, OS/hypervisor, network identities, authenticated access references, storage, templates, constraints, observed guests, recovery and dated evidence. Unknown facts stay explicitly unknown until verified. Template IDs and guest IDs belong to the selected host's record, not global constants.

Use both machine and service records to select placement, then check live capacity. Health is distinct from capacity and readiness: a TCP probe establishes reachability only; stale/unknown does not establish downtime. Loopback URLs refer to the named host. Resolve current ownership and leases through the registered provisioner when one exists.

## Create, update and retire

For a machine, use a `machine-<name>` ID, capability `machine`, and `resources.kind: "machine"`. Store setup and machine-specific runbooks in `resources`; include observation timestamps and clearly separate planned changes from installed capabilities. Link service records through their physical `host`, deployment identity and dependencies. Store credential references, never values.

Each spec requires `name`, `host`, `owner`, `capabilities`, `endpoints`, `lifecycle`, `runbook` and `dependencies`. Use `credential_ref`, `deployment`, `resources`, `notes`, `backup`, `config_backup` and `probe` as applicable. Endpoint URLs must be credential-free. Write a spec JSON file, not the surrounding response envelope.

```sh
registry.py put RECORD_ID SPEC.json --revision 0
registry.py get RECORD_ID
registry.py put RECORD_ID SPEC.json --revision CURRENT_REVISION
registry.py history RECORD_ID
```

Revision 0 creates; updates require the fetched revision. Preserve unrelated fields. On conflict, fetch and deliberately merge; never blindly overwrite with a newer revision. Read back the record after mutation. Update both host setup and affected service records after changes; adding a machine, template or service does not require a skill edit.

After functional validation, publish bounded evidence:

```sh
registry.py observe RECORD_ID --status healthy --evidence 'actual check and result' --ttl 300
```

Use unhealthy/unknown when appropriate. Updating a record invalidates old health evidence. Configure recurring probes or reporters and verify a scheduled run; a one-time observation is not monitoring.

HTTP probes use `{"url":"https://example.internal/healthz","vantage":"registry","expected_status":200}`. They must be credential-free and return directly without redirects. TCP probes use `{"tcp_host":"HOST","tcp_port":22,"vantage":"registry"}` and establish connectivity only. Discover any additional supported reporter vantage points and bounded local checks through the registry service's runbook. Records must not provide arbitrary executable check commands. Verify readiness failures are not hidden by HTTP 200. Registration is explicit: deployment agents maintain records; process discovery is not automatic.

After authorized teardown, use `registry.py retire RECORD_ID --revision CURRENT_REVISION`; history remains. `list --include-retired` includes retired entries. Check dependencies before retiring a host or shared service. Archive old evidence rather than presenting it as current setup or ongoing authorization.

## Backups and registry recovery

Follow the [backup SOP](backups.md) for machine and service configuration. Keep actual locations, job identities, cadence, retention, coverage, encryption references and restore evidence in their registry records. `config_backup` supplies dashboard fields; `backup` and runbooks describe recovery details.

Discover the registry's own deployment, source/runbook, scheduled checks, backup/export jobs and recovery through its record. Verify catalog exports include machine resources and inline runbooks, not just service endpoints. Preserve TLS/private configuration separately from credential-free catalog exports.

A registry outage must not require an alternate inventory in the skill. Maintain a protected recovery bundle containing an exported registry catalog and the private bootstrap/access material needed to restore it, accessible independently of the registry host. Use that export as an explicitly dated recovery snapshot, verify identities and restore the authoritative registry before ordinary placement decisions. Do not guess hosts or endpoints from old transcripts. Revalidate changed addresses and reissue certificates/update private client configuration when necessary; never disable TLS verification.
