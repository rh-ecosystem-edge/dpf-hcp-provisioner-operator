#!/bin/bash
# Generate rpms.lock.yaml (Konflux/hermeto RPM lockfile format) for the packages listed in
# rpms.in.yaml, using only dnf from the UBI10 builder image.
#
# Usage (from the repo root):
#   podman run --rm -v "$PWD/rpm-prefetching:/work:Z" -w /work \
#     registry.access.redhat.com/ubi10/ubi:latest bash ./generate-lock.sh
#
# The package set is what `dnf install` adds on top of the base image (resolved on x86_64);
# the same package names are then resolved for every architecture in ARCHES.
set -euo pipefail

PACKAGES=(rust cargo gcc glibc-static)
ARCHES=(aarch64 x86_64)
REPOFILE=/work/ubi.repo
OUT=/work/rpms.lock.yaml

cp "${REPOFILE}" /etc/yum.repos.d/ubi.repo

# Names of the packages a `dnf install` of PACKAGES would add to the base image.
mapfile -t NAMES < <(
  dnf install -y --downloadonly --downloaddir /tmp/resolve "${PACKAGES[@]}" >/dev/null
  for f in /tmp/resolve/*.rpm; do rpm -qp --qf '%{NAME}\n' "$f"; done | sort -u
)

{
  echo "---"
  echo "lockfileVersion: 1"
  echo "lockfileVendor: redhat"
  echo "arches:"
  for arch in "${ARCHES[@]}"; do
    echo "- arch: ${arch}"
    echo "  packages:"
    dir="/tmp/rpms-${arch}"
    mkdir -p "${dir}"
    dnf download -q --forcearch "${arch}" --arch "${arch},noarch" --destdir "${dir}" "${NAMES[@]}" >/dev/null
    for f in $(ls "${dir}"/*.rpm | sort); do
      name=$(rpm -qp --qf '%{NAME}' "$f")
      nevra=$(rpm -qp --qf '%{NAME}-%{EPOCHNUM}:%{VERSION}-%{RELEASE}.%{ARCH}' "$f")
      evr=$(rpm -qp --qf '%|EPOCH?{%{EPOCH}:}|%{VERSION}-%{RELEASE}' "$f")
      srpm=$(rpm -qp --qf '%{SOURCERPM}' "$f")
      size=$(stat -c %s "$f")
      sum=$(sha256sum "$f" | cut -d' ' -f1)
      url=$(dnf repoquery -q --forcearch "${arch}" --latest-limit 1 --location "${nevra}" | head -1)
      repoid=$(dnf repoquery -q --forcearch "${arch}" --latest-limit 1 --qf '%{repoid}\n' "${nevra}" | head -1)
      echo "  - url: ${url}"
      echo "    repoid: ${repoid}"
      echo "    size: ${size}"
      echo "    checksum: sha256:${sum}"
      echo "    name: ${name}"
      echo "    evr: ${evr}"
      echo "    sourcerpm: ${srpm}"
    done
    echo "  source: []"
    echo "  module_metadata: []"
  done
} > "${OUT}"
echo "wrote ${OUT}"
