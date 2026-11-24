# Registry platform release

The registry, dashboard and allocator ship as one Linux release. Run the same
release on a disposable VM before activating it in production. The catalog and
lease ledger have separate service accounts, persistent state and credentials.
The allocator's enrolled providers and workers can run on other machines.

## Assemble the release

Build the binaries from one source checkout on the target Linux architecture.
The three Cargo commands can share an absolute `CARGO_TARGET_DIR`. Build the
[dashboard assets](../README.md#deployment) in the same checkout.

```sh
export CARGO_TARGET_DIR="$(pwd)/target/platform"
cargo build --locked --release --manifest-path registry/rust/Cargo.toml --bin lab-registry
cargo build --locked --release --manifest-path dashboard/rust/Cargo.toml --bin lab-dashboard
cargo build --locked --release --manifest-path allocator/rust/Cargo.toml \
  --bin lab-allocator --bin lab-checkpoint-worker --bin lab-lease-exporter \
  --bin lab-lease-alert --bin lab-restore-workspace --bin lab-cold-capture --bin lab-tart-capture
mkdir -p ./releases
release=$(deploy/platform-release.sh package ./releases "$CARGO_TARGET_DIR/release" dashboard)
deploy/platform-release.sh verify "./releases/$release"
```

The release ID is the checksum of the manifest for the binaries, browser
assets and unit definitions. Transfer the same release directory from staging to production. The
host's configuration, credentials, ledgers and workspaces stay outside it.

## Configure and activate

- Provision the `lab-registry`, `lab-dashboard` and `lab-allocator` service
  accounts and private configuration. The sample units use
  `/etc/lab-registry/config.json`, `/etc/lab-allocator/config.json` and the
  dashboard configuration directory at `/opt/machine-dashboard/private`.
  Give each account access to its own config, state and credentials. Configure
  at least one static task credential. An operator recovery identity can
  supply it when routine agents use short-lived grants from a separate issuer.
- On first installation, install the bundled [target](lab-platform.target) and its three service units
  in `/etc/systemd/system`. Keep host-specific settings in systemd drop-ins.
  Remove old drop-ins that pin processes or assets to previous releases.
  When migrating individually managed services, preserve their definitions
  and rollback paths, then stop
  and disable the old units at cutover so they cannot start alongside the
  target. Leave other provider and storage workloads running.
- Point local checkpoint, export and alert workers and their capture commands
  at the bundled executables under `/opt/lab-platform-releases/current/bin`.
  Set `PartOf=lab-platform.target` on these local services and timers. Have the
  target pull in the checkpoint worker and the enabled timers so they switch
  releases with the allocator.
- Follow the [allocator enrollment and handoff
  procedures](../allocator/README.md#fenced-replacement-controller). Verify
  that a prior controller has lost mutation authority before enrolling or
  transferring its pools. Recover a ledger using its independently held
  revision witness. Give the allocator write access to its state directory and
  to the witness directory outside the ledger's restore set. Activate a compatible binary against
  the current ledger during rollback.

Put the root-owned release directory under `/opt/lab-platform-releases`. Add
the host-local link `dashboard/private -> /opt/machine-dashboard/private` inside
that directory. Run `systemctl daemon-reload` after installing or updating
units. Then activate with the site's authenticated health check.

```sh
sudo "/opt/lab-platform-releases/$release/deploy/platform-release.sh" activate "$release" /path/to/site-health-check
sudo systemctl enable lab-platform.target
```

The check runs after all three units start. It must make an authenticated
catalog read, an authenticated allocator read and a dashboard request through
the normal access path. Open the authenticated inventory stream or refresh
the dashboard before asserting live lease status. If the check fails, activation restores the previous
release pointer and its unit definitions, then starts the target again.
Subsequent activations install the bundled units and reload systemd. On a
first-time migration, use the preserved unit definitions to recover the previous installation if activation
fails. Verify the new release with real lease acquisition, renewal, checkpoint
recovery and release, and confirm the dashboard shows current inventory and
lease ownership. Record the deployed release ID, backup and qualification
evidence in the registry.

Run `sh tests/platform_release.sh "$CARGO_TARGET_DIR/release" dashboard` with
the built artifacts to check that packaging and verification reject incomplete
or changed releases.
