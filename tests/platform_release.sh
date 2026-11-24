#!/bin/sh
# Exercise the release tool with the actual Linux binaries and dashboard build.
set -eu
if [ "$#" -ne 2 ]; then
    echo "Usage: $0 RUST_ARTIFACTS DASHBOARD_ASSETS" >&2
    exit 2
fi
tool="$(dirname "$0")/../deploy/platform-release.sh"
scratch=$(mktemp -d)
trap 'rm -rf -- "$scratch"' EXIT

release=$("$tool" package "$scratch" "$1" "$2")
"$tool" verify "$scratch/$release"

# Both executables must be in a single verified release.
test -x "$scratch/$release/bin/lab-registry"
test -x "$scratch/$release/bin/lab-allocator"
test -x "$scratch/$release/bin/lab-dashboard"
test -s "$scratch/$release/deploy/lab-platform.target"
test ! -e "$scratch/$release/dashboard/private"

cp -R "$scratch/$release" "$scratch/candidate"
rm "$scratch/candidate/bin/lab-allocator"
if "$tool" verify "$scratch/candidate"; then
    echo "Accepted a release without its allocator" >&2
    exit 1
fi

cp "$scratch/$release/bin/lab-allocator" "$scratch/candidate/bin/lab-allocator"
printf 'change' >> "$scratch/candidate/dashboard/dashboard-react.js"
if "$tool" verify "$scratch/candidate"; then
    echo "Accepted a dashboard bundle with a mismatched checksum" >&2
    exit 1
fi

printf '%s\n' 'Complete platform release verified; partial and damaged releases rejected'
