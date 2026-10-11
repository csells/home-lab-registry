# Superseded implementation and example checks

These files preserve the original service-only dashboard and fixed local checks.
They are not used by the current dashboard or installed automatically. The current
implementation is in `dashboard/` and `registry/`; the daemon is in `agent/`.

The checks illustrate fixed, bounded commands for specific applications; adapt them
to a deployment before use. This is source history, not live service inventory.
Registry test fixture differences on the deployed copy are site names only; the
portable tests remain in `registry/test_registry.py`.

Private configuration, inventory, runbooks containing site details, and credentials
are deliberately excluded. Recover those from the central encrypted backup.
