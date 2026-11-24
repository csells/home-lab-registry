#!/bin/sh
# Build a credential-free release directory and switch all local platform services to it.
set -eu

root=/opt/lab-platform-releases
target=lab-platform.target
units="$target lab-registry.service lab-dashboard.service lab-allocator.service"
binaries='lab-registry lab-dashboard lab-allocator lab-checkpoint-worker lab-lease-exporter lab-lease-alert lab-restore-workspace lab-cold-capture lab-tart-capture'
assets='index.html app.css leases.css resource-graph.css dashboard-react.js vendor/elk-worker.min.js'

fail() {
    printf '%s\n' "$*" >&2
    exit 1
}

manifest() (
    cd -- "$1"
    for directory in bin dashboard dashboard/vendor deploy; do
        [ -d "$directory" ] && [ ! -L "$directory" ] ||
            fail "Missing release directory: $directory"
    done
    for name in $binaries; do
        file=bin/$name
        [ -f "$file" ] && [ -x "$file" ] && [ ! -L "$file" ] && [ -s "$file" ] ||
            fail "Missing executable in release: $file"
        sha256sum -- "$file" || exit
    done
    for name in $assets; do
        file=dashboard/$name
        [ -f "$file" ] && [ ! -L "$file" ] && [ -s "$file" ] ||
            fail "Missing dashboard asset in release: $file"
        sha256sum -- "$file" || exit
    done
    for name in $units platform-release.sh; do
        file=deploy/$name
        [ -f "$file" ] && [ ! -L "$file" ] && [ -s "$file" ] ||
            fail "Missing deployment file in release: $file"
        sha256sum -- "$file" || exit
    done
)

release_id() {
    sum=$(sha256sum < "$1/sha256")
    printf '%s\n' "${sum%% *}"
}

verify() {
    actual=$(manifest "$1") || exit
    [ -f "$1/sha256" ] && [ ! -L "$1/sha256" ] || fail 'Missing release checksum manifest'
    printf '%s\n' "$actual" | cmp -s - "$1/sha256" ||
        fail 'The release checksum manifest does not match its artifacts'
}

package() (
    [ "$#" -eq 3 ] || fail 'Usage: platform-release.sh package OUTPUT RUST_ARTIFACTS DASHBOARD_ASSETS'
    output=$1
    rust=$2
    dashboard=$3
    deploy=$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd)
    scratch=$(mktemp -d "$output/.bundle.XXXXXXXX")
    trap 'rm -rf -- "$scratch"' EXIT
    chmod 0755 "$scratch"
    install -d -m 0755 -- "$scratch/bin" "$scratch/dashboard" "$scratch/dashboard/vendor" "$scratch/deploy"
    for name in $units; do
        install -m 0644 -- "$deploy/$name" "$scratch/deploy/$name"
    done
    install -m 0755 -- "$deploy/platform-release.sh" "$scratch/deploy/platform-release.sh"
    for name in $binaries; do
        install -m 0755 -- "$rust/$name" "$scratch/bin/$name"
    done
    for name in $assets; do
        install -m 0644 -- "$dashboard/$name" "$scratch/dashboard/$name"
    done
    manifest "$scratch" > "$scratch/sha256"
    id=$(release_id "$scratch")
    if [ -e "$output/$id" ]; then
        verify "$output/$id"
        [ "$(release_id "$output/$id")" = "$id" ] || fail 'Existing release has different contents'
    else
        mv -T -- "$scratch" "$output/$id"
    fi
    printf '%s\n' "$id"
)

activate() (
    [ "$#" -ge 2 ] || fail 'Usage: platform-release.sh activate RELEASE_ID HEALTH_CHECK [ARG ...]'
    [ "$(id -u)" -eq 0 ] || fail 'Run activation as root'
    next=$1
    shift
    case "$next" in
        ''|*[!0123456789abcdef]*) fail 'Expected a hexadecimal release ID' ;;
    esac
    [ "${#next}" -eq 64 ] || fail 'Expected a full SHA-256 release ID'
    exec 9>"$root/.activate.lock"
    flock -x 9

    verify "$root/$next"
    [ "$(release_id "$root/$next")" = "$next" ] || fail 'Release ID does not match the bundle'
    [ -d /opt/machine-dashboard/private ] || fail 'Dashboard configuration is missing'
    [ "$(readlink "$root/$next/dashboard/private")" = /opt/machine-dashboard/private ] ||
        fail 'Link the host dashboard configuration to dashboard/private before activation'
    previous=$(readlink "$root/current" || :)
    if [ -n "$previous" ]; then
        verify "$root/$previous"
        [ "$(release_id "$root/$previous")" = "$previous" ] || fail 'The current link is not a release ID'
    else
        [ ! -e "$root/current" ] || fail 'The current release path must be a symbolic link'
    fi
    for unit in $units; do
        cmp -s -- "/etc/systemd/system/$unit" "$root/${previous:-$next}/deploy/$unit" ||
            fail "The installed unit differs from the current release: $unit"
    done

    install_units() {
        for unit in $units; do
            install -m 0644 -- "$root/$1/deploy/$unit" "/etc/systemd/system/$unit" || return
        done
        systemctl daemon-reload
    }

    scratch=$(mktemp -d "$root/.activation.XXXXXXXX")
    switching=no
    # shellcheck disable=SC2329  # Invoked by the EXIT trap.
    finish() {
        status=$?
        trap - EXIT HUP INT TERM
        if [ "$switching" = yes ]; then
            if ! systemctl stop "$target"; then
                printf '%s\n' 'Could not stop the candidate services for rollback' >&2
            elif [ -n "$previous" ]; then
                if mv -Tf -- "$scratch/previous" "$root/current" &&
                    install_units "$previous" && systemctl start "$target"; then
                    printf 'Restored previous release %s\n' "$previous" >&2
                else
                    printf 'Could not restart the previous release: %s\n' "$previous" >&2
                fi
            else
                rm -f -- "$root/current"
            fi
        fi
        rm -rf -- "$scratch"
        exit "$status"
    }
    trap finish EXIT
    trap 'exit 1' HUP INT TERM
    ln -s -- "$next" "$scratch/next"
    if [ -n "$previous" ]; then
        ln -s -- "$previous" "$scratch/previous"
    fi

    switching=yes
    systemctl stop "$target"
    mv -Tf -- "$scratch/next" "$root/current"
    install_units "$next"
    systemctl start "$target"
    "$@" || fail "Health check failed for release $next"
    switching=no
    printf 'Activated %s\n' "$next"
)

case "${1:-}" in
    package) shift; package "$@" ;;
    verify) [ "$#" -eq 2 ] || fail 'Usage: platform-release.sh verify RELEASE_DIRECTORY'; verify "$2" ;;
    activate) shift; activate "$@" ;;
    *) fail 'Usage: platform-release.sh {package|verify|activate} ...' ;;
esac
