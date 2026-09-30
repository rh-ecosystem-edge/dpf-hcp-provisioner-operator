# Releasing this Helm Chart to registry.redhat.io

## Why this procedure exists

The chart's `values.yaml` normally points at the Konflux dev build for its branch (e.g.
`quay.io/redhat-user-workloads/dpu-kit-for-nvidia-operator-tenant/dpf-hcp-provisioner-rhel10-operator-4-22`),
with `image.tag` left empty so it falls back to `.Chart.AppVersion` (set to the commit SHA
automatically by Konflux's `build-helm-chart-oci-ta` task at package time). This lets
`helm upgrade --install` work out of the box during development, with no `--set image.*`
required.

**The Konflux release pipeline (`rh-push-helm-chart-to-registry-redhat-io`) does not rewrite
chart content.** It only relocates the already-built, already-tested chart OCI artifact to
`registry.redhat.io` via a byte-for-byte copy (`cosign copy` / `oras cp`), retagging it in the
process. It never unpacks the `.tgz`, and it never touches `values.yaml` or `Chart.yaml`.

This means: whatever `image.repository` / `image.tag` are in `values.yaml` **at the exact
commit that Release Engineering selects for release** is exactly what ships to customers,
unchanged. If that commit still points at the internal
`quay.io/redhat-user-workloads/...` dev registry, that is what customers would get — which
is broken, since that registry is not customer-accessible.

## Procedure

This is a two-step manual procedure on the release branch (e.g. `release-4.22`), performed
around each GA/z-stream release.

### 1. Before cutting the release

Open a PR against the release branch that flips `values.yaml` to the production image:

```yaml
image:
  repository: registry.redhat.io/dpu-kit-for-nvidia/dpf-hcp-provisioner-rhel10-operator
  tag: "4.22"
```

Merge it. Konflux's normal `chart-push` pipeline builds that exact commit and pushes the
resulting chart to `quay.io/redhat-user-workloads/dpu-kit-for-nvidia-operator-tenant/chart-4-22:<commit-sha>`
— this build now has the production content already baked in, because packaging just
reflects whatever is literally in the source at that commit.

Hand off **this specific commit/snapshot** to Release Engineering. When
`rh-push-helm-chart-to-registry-redhat-io` copies it to `registry.redhat.io`, the bytes it
copies already say `registry.redhat.io/dpu-kit-for-nvidia/dpf-hcp-provisioner-rhel10-operator:4.22`,
so the shipped chart is correct.

### 2. Right after the release is out

Open a follow-up PR that flips `values.yaml` back to the dev image, so ongoing z-stream
development (e.g. 4.22.1, 4.22.2, ...) on the release branch continues to get a fresh dev
build per commit without needing `--set` overrides:

```yaml
image:
  repository: quay.io/redhat-user-workloads/dpu-kit-for-nvidia-operator-tenant/dpf-hcp-provisioner-rhel10-operator-4-22
  tag: ""
```

Merge it. From this point on, every push to the branch builds and pushes a chart with the
correct dev image + commit-sha tag again, until the next release cutoff repeats step 1.

## Checklist

- [ ] PR #1 merged: `values.yaml` points at `registry.redhat.io/...` with the release tag
- [ ] Confirmed the Konflux `chart-push` build for that commit succeeded (check
      `quay.io/.../chart-4-22:<commit-sha>` exists and contains the expected content --
      `helm pull` + `tar -xzO .../values.yaml` to double check if unsure)
- [ ] Handed off the commit/snapshot to Release Engineering
- [ ] Release confirmed live on `registry.redhat.io`
- [ ] PR #2 merged: `values.yaml` reverted to the dev image for continued development

## Related

- [`values.yaml`](values.yaml) -- see the `image:` block comments
- [`values-production.yaml`](values-production.yaml) -- optional local override for testing
  against a released image without touching `values.yaml`
