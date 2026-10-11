# Central configuration export

`collect.py` is the Backrest export hook. Install alongside `registry/client.py`
and `registry/resolve.py`, preserving their relative layout. The runtime base is
`/opt/backrest`; deployment manifests and credentials remain private.

Sources are resolved from fresh registry presence for each export, with stable SSH
host-key aliases. See [registration](../deploy/metrics/REGISTRATION.md). Scheduling,
encryption, retention and the Google Drive backend remain Backrest/restic/rclone
responsibilities. This module adds no per-machine backup timers or retained copies.
