//! Flux objects for tests, taken from Flux 2.9 on kind (`script/flux-dev.sh`, trimmed and with
//! a few fields added to cover more cases) and, for older API versions, shaped like their
//! `v1beta2`/`v2beta1` CRDs.

use serde_json::{Value, json};

pub fn kustomization_ready() -> Value {
    json!({
        "apiVersion": "kustomize.toolkit.fluxcd.io/v1",
        "kind": "Kustomization",
        "metadata": {"name": "podinfo", "namespace": "flux-demo", "generation": 2,
            "uid": "6f0b6f7e-1", "creationTimestamp": "2026-10-07T10:33:50Z",
            "finalizers": ["finalizers.fluxcd.io"]},
        "spec": {
            "force": false, "interval": "10m", "path": "./kustomize", "prune": true,
            "postBuild": {"substitute": {"cluster_env": "dev"},
                "substituteFrom": [{"kind": "Secret", "name": "podinfo-substitutions", "optional": true}]},
            "sourceRef": {"kind": "GitRepository", "name": "podinfo"},
            "targetNamespace": "flux-podinfo"
        },
        "status": {
            "conditions": [{"lastTransitionTime": "2026-10-07T10:36:30Z",
                "message": "Applied revision: master@sha1:3e0ff8ae123b710bc91de1315cba0f996a8896c2",
                "observedGeneration": 2, "reason": "ReconciliationSucceeded", "status": "True", "type": "Ready"}],
            "history": [{"digest": "sha256:c635f7b20087b5840dcf83c26129dbcde4c1cc9de5e76134a32467e6588da888",
                "firstReconciled": "2026-10-07T10:36:30Z", "lastReconciled": "2026-10-07T10:36:30Z",
                "lastReconciledDuration": "170.6625ms", "lastReconciledStatus": "ReconciliationSucceeded",
                "metadata": {"revision": "master@sha1:3e0ff8ae123b710bc91de1315cba0f996a8896c2"},
                "totalReconciliations": 1}],
            "inventory": {"entries": [
                {"id": "flux-podinfo_podinfo__Service", "v": "v1"},
                {"id": "flux-podinfo_podinfo_apps_Deployment", "v": "v1"},
                {"id": "flux-podinfo_podinfo_autoscaling_HorizontalPodAutoscaler", "v": "v2"}
            ]},
            "lastAppliedRevision": "master@sha1:3e0ff8ae123b710bc91de1315cba0f996a8896c2",
            "lastAttemptedRevision": "master@sha1:3e0ff8ae123b710bc91de1315cba0f996a8896c2",
            "observedGeneration": 2
        }
    })
}

pub fn kustomization_failed() -> Value {
    json!({
        "apiVersion": "kustomize.toolkit.fluxcd.io/v1",
        "kind": "Kustomization",
        "metadata": {"name": "broken", "namespace": "flux-demo", "generation": 1,
            "creationTimestamp": "2026-10-07T10:33:50Z"},
        "spec": {"interval": "5m", "retryInterval": "1m", "path": "./does-not-exist", "prune": true,
            "sourceRef": {"kind": "GitRepository", "name": "podinfo"}},
        "status": {
            "conditions": [
                {"lastTransitionTime": "2026-10-07T10:36:53Z",
                 "message": "Fetching manifests for revision master@sha1:3e0ff8ae123b710bc91de1315cba0f996a8896c2 with a timeout of 4m30s",
                 "observedGeneration": 1, "reason": "ProgressingWithRetry", "status": "True", "type": "Reconciling"},
                {"lastTransitionTime": "2026-10-07T10:36:53Z",
                 "message": "kustomization path not found: stat /tmp/kustomization-1709002560/does-not-exist: no such file or directory",
                 "observedGeneration": 1, "reason": "ArtifactFailed", "status": "False", "type": "Ready"}
            ],
            "observedGeneration": -1
        }
    })
}

pub fn kustomization_suspended() -> Value {
    json!({
        "apiVersion": "kustomize.toolkit.fluxcd.io/v1",
        "kind": "Kustomization",
        "metadata": {"name": "paused", "namespace": "flux-demo", "generation": 2,
            "creationTimestamp": "2026-10-01T10:33:50Z"},
        "spec": {"interval": "10m", "path": "./kustomize", "prune": true, "suspend": true,
            "targetNamespace": "flux-paused", "sourceRef": {"kind": "GitRepository", "name": "podinfo"}},
        "status": {
            "conditions": [{"lastTransitionTime": "2026-10-01T10:36:30Z",
                "message": "Applied revision: master@sha1:3e0ff8ae123b710bc91de1315cba0f996a8896c2",
                "observedGeneration": 1, "reason": "ReconciliationSucceeded", "status": "True", "type": "Ready"}],
            "lastAppliedRevision": "master@sha1:3e0ff8ae123b710bc91de1315cba0f996a8896c2",
            "observedGeneration": 1
        }
    })
}

pub fn kustomization_waiting() -> Value {
    json!({
        "apiVersion": "kustomize.toolkit.fluxcd.io/v1",
        "kind": "Kustomization",
        "metadata": {"name": "apps-late", "namespace": "flux-demo", "generation": 1},
        "spec": {"dependsOn": [{"name": "broken"}], "interval": "10m", "path": "./kustomize",
            "prune": true, "retryInterval": "1m", "targetNamespace": "flux-chain",
            "sourceRef": {"kind": "GitRepository", "name": "podinfo"}},
        "status": {
            "conditions": [{"lastTransitionTime": "2026-10-07T10:33:53Z",
                "message": "dependency 'flux-demo/broken' is not ready", "observedGeneration": 1,
                "reason": "DependencyNotReady", "status": "False", "type": "Ready"}],
            "observedGeneration": -1
        }
    })
}

pub fn helm_release_v2() -> Value {
    json!({
        "apiVersion": "helm.toolkit.fluxcd.io/v2",
        "kind": "HelmRelease",
        "metadata": {"name": "podinfo-helm", "namespace": "flux-demo", "generation": 1},
        "spec": {
            "chart": {"spec": {"chart": "podinfo", "reconcileStrategy": "ChartVersion",
                "sourceRef": {"kind": "HelmRepository", "name": "podinfo"}, "version": "6.x"}},
            "install": {"remediation": {"retries": 3}},
            "interval": "10m",
            "upgrade": {"remediation": {"remediateLastFailure": true, "retries": 3}},
            "values": {"resources": {"requests": {"cpu": "10m", "memory": "32Mi"}}},
            "valuesFrom": [{"kind": "ConfigMap", "name": "podinfo-values"},
                {"kind": "Secret", "name": "podinfo-secret-values", "valuesKey": "values.yaml"}]
        },
        "status": {
            "conditions": [
                {"lastTransitionTime": "2026-10-07T10:34:02Z",
                 "message": "Helm install succeeded for release flux-demo/podinfo-helm.v1 with chart podinfo@6.15.0",
                 "observedGeneration": 1, "reason": "InstallSucceeded", "status": "True", "type": "Ready"},
                {"lastTransitionTime": "2026-10-07T10:34:02Z",
                 "message": "Helm install succeeded for release flux-demo/podinfo-helm.v1 with chart podinfo@6.15.0",
                 "observedGeneration": 1, "reason": "InstallSucceeded", "status": "True", "type": "Released"}
            ],
            "helmChart": "flux-demo/flux-demo-podinfo-helm",
            "history": [{"action": "install", "apiVersion": "v2", "appVersion": "6.15.0", "chartName": "podinfo",
                "chartVersion": "6.15.0", "configDigest": "sha256:90917a03", "digest": "sha256:235cd829",
                "firstDeployed": "2026-10-07T10:33:52Z", "lastDeployed": "2026-10-07T10:33:52Z",
                "name": "podinfo-helm", "namespace": "flux-demo", "status": "deployed", "version": 1}],
            "inventory": {"entries": [
                {"id": "flux-demo_podinfo-helm__Service", "v": "v1"},
                {"id": "flux-demo_podinfo-helm_apps_Deployment", "v": "v1"}
            ]},
            "lastAttemptedReleaseAction": "install",
            "lastAttemptedRevision": "6.15.0",
            "observedGeneration": 1,
            "storageNamespace": "flux-demo"
        }
    })
}

/// A `v2beta1` HelmRelease: `lastAppliedRevision`, `lastReleaseRevision`, failure counters,
/// no `history`.
pub fn helm_release_v2beta1() -> Value {
    json!({
        "apiVersion": "helm.toolkit.fluxcd.io/v2beta1",
        "kind": "HelmRelease",
        "metadata": {"name": "podinfo", "namespace": "apps", "generation": 4},
        "spec": {"interval": "5m", "releaseName": "web", "targetNamespace": "web",
            "chart": {"spec": {"chart": "podinfo", "version": "6.5.x",
                "sourceRef": {"kind": "HelmRepository", "name": "podinfo", "namespace": "flux-system"}}},
            "install": {"remediation": {"retries": -1}}},
        "status": {
            "conditions": [{"type": "Ready", "status": "True", "reason": "ReconciliationSucceeded",
                "message": "Release reconciliation succeeded", "lastTransitionTime": "2025-01-01T00:00:00Z"}],
            "failures": 2, "installFailures": 0, "upgradeFailures": 1,
            "helmChart": "flux-system/apps-podinfo",
            "lastAppliedRevision": "6.5.4", "lastAttemptedRevision": "6.5.4",
            "lastReleaseRevision": 3, "observedGeneration": 4
        }
    })
}

pub fn helm_release_stalled() -> Value {
    json!({
        "apiVersion": "helm.toolkit.fluxcd.io/v2",
        "kind": "HelmRelease",
        "metadata": {"name": "redis", "namespace": "data", "generation": 2},
        "spec": {"interval": "10m",
            "chart": {"spec": {"chart": "redis", "version": "19.x",
                "sourceRef": {"kind": "HelmRepository", "name": "bitnami"}}},
            "install": {"remediation": {"retries": 3}}},
        "status": {
            "conditions": [
                {"type": "Stalled", "status": "True", "reason": "RetriesExceeded",
                 "message": "Failed to install after 4 attempt(s)", "observedGeneration": 2},
                {"type": "Ready", "status": "False", "reason": "InstallFailed",
                 "message": "Helm install failed for release data/redis with chart redis@19.6.4: context deadline exceeded",
                 "observedGeneration": 2}
            ],
            "installFailures": 4, "failures": 4, "observedGeneration": 2,
            "history": [{"chartName": "redis", "chartVersion": "19.6.4", "name": "redis", "namespace": "data",
                "status": "failed", "version": 1, "firstDeployed": "2026-10-06T08:00:00Z",
                "lastDeployed": "2026-10-06T08:00:00Z", "digest": "sha256:aa", "configDigest": "sha256:bb"}]
        }
    })
}

/// A HelmRelease on an OCI chart through `spec.chartRef`.
pub fn helm_release_chart_ref() -> Value {
    json!({
        "apiVersion": "helm.toolkit.fluxcd.io/v2",
        "kind": "HelmRelease",
        "metadata": {"name": "podinfo-oci", "namespace": "apps", "generation": 1},
        "spec": {"interval": "10m", "chartRef": {"kind": "OCIRepository", "name": "podinfo", "namespace": "flux-system"}},
        "status": {"conditions": [{"type": "Ready", "status": "True", "reason": "UpgradeSucceeded", "message": "ok"}]}
    })
}

pub fn git_repository() -> Value {
    json!({
        "apiVersion": "source.toolkit.fluxcd.io/v1",
        "kind": "GitRepository",
        "metadata": {"name": "podinfo", "namespace": "flux-demo", "generation": 1},
        "spec": {"interval": "5m", "ref": {"branch": "master"}, "timeout": "60s",
            "url": "https://github.com/stefanprodan/podinfo"},
        "status": {
            "artifact": {"digest": "sha256:903f57ee8629b67dbd50bba241633df24168efbd09c2f2cab02ba8172360a954",
                "lastUpdateTime": "2026-10-07T10:33:52Z",
                "path": "gitrepository/flux-demo/podinfo/3e0ff8ae123b710bc91de1315cba0f996a8896c2.tar.gz",
                "revision": "master@sha1:3e0ff8ae123b710bc91de1315cba0f996a8896c2", "size": 402545,
                "url": "http://source-controller.flux-system.svc.cluster.local./gitrepository/flux-demo/podinfo/3e0ff8ae123b710bc91de1315cba0f996a8896c2.tar.gz"},
            "conditions": [
                {"lastTransitionTime": "2026-10-07T10:33:52Z",
                 "message": "stored artifact for revision 'master@sha1:3e0ff8ae123b710bc91de1315cba0f996a8896c2'",
                 "observedGeneration": 1, "reason": "Succeeded", "status": "True", "type": "Ready"},
                {"lastTransitionTime": "2026-10-07T10:33:52Z",
                 "message": "stored artifact for revision 'master@sha1:3e0ff8ae123b710bc91de1315cba0f996a8896c2'",
                 "observedGeneration": 1, "reason": "Succeeded", "status": "True", "type": "ArtifactInStorage"}
            ],
            "observedGeneration": 1
        }
    })
}

/// A `v1beta2` GitRepository as Flux 0.x wrote it: `checksum` instead of `digest`, the old
/// `<branch>/<sha>` revision format (Flux 2.x writes `digest` and `<ref>@sha1:<sha>` at every
/// version), and credentials in the URL (which Kubyl strips).
pub fn git_repository_v1beta2() -> Value {
    json!({
        "apiVersion": "source.toolkit.fluxcd.io/v1beta2",
        "kind": "GitRepository",
        "metadata": {"name": "infra", "namespace": "flux-system", "generation": 3},
        "spec": {"interval": "1m", "ref": {"tag": "v1.2.0"},
            "url": "https://deploy:hunter2@gitlab.example.com/platform/infra.git",
            "secretRef": {"name": "infra-auth"}},
        "status": {
            "artifact": {"checksum": "1f7a0e2c9b", "lastUpdateTime": "2024-03-01T09:00:00Z",
                "revision": "main/9f3c2a1b2c3d4e5f60718293a4b5c6d7e8f90123"},
            "conditions": [{"type": "Ready", "status": "True", "reason": "Succeeded",
                "message": "stored artifact for revision 'main/9f3c2a1b2c3d4e5f60718293a4b5c6d7e8f90123'",
                "lastTransitionTime": "2024-03-01T09:00:00Z"}],
            "observedGeneration": 3
        }
    })
}

pub fn oci_repository() -> Value {
    json!({
        "apiVersion": "source.toolkit.fluxcd.io/v1",
        "kind": "OCIRepository",
        "metadata": {"name": "podinfo-manifests", "namespace": "flux-demo", "generation": 1},
        "spec": {"interval": "30m", "provider": "generic", "ref": {"tag": "latest"}, "timeout": "60s",
            "url": "oci://ghcr.io/stefanprodan/manifests/podinfo"},
        "status": {
            "artifact": {"digest": "sha256:1034e35fc4362f6fd7002e5cc3e4253d4e1f2d438c9d0ee1c7bb9cfbe9f44a9d",
                "lastUpdateTime": "2026-10-07T10:33:53Z",
                "metadata": {"org.opencontainers.image.created": "2026-08-31T15:40:18Z",
                    "org.opencontainers.image.revision": "6.15.0@sha1:dd507173b7b75b2312a36cabe0de5f09c1ce69c8",
                    "org.opencontainers.image.source": "https://github.com/stefanprodan/podinfo"},
                "revision": "latest@sha256:87815bbd58f5bfd5ad9deabb76dbd0615562cd0df288759c6e50ee54b2860b0c",
                "size": 1112},
            "conditions": [{"lastTransitionTime": "2026-10-07T10:33:53Z",
                "message": "stored artifact for digest 'latest@sha256:87815bbd58f5bfd5ad9deabb76dbd0615562cd0df288759c6e50ee54b2860b0c'",
                "observedGeneration": 1, "reason": "Succeeded", "status": "True", "type": "Ready"}],
            "observedGeneration": 1
        }
    })
}

pub fn helm_repository() -> Value {
    json!({
        "apiVersion": "source.toolkit.fluxcd.io/v1",
        "kind": "HelmRepository",
        "metadata": {"name": "podinfo", "namespace": "flux-demo", "generation": 1},
        "spec": {"interval": "30m", "provider": "generic", "url": "https://stefanprodan.github.io/podinfo"},
        "status": {
            "artifact": {"digest": "sha256:11752bf8beeb6dc33feac6184c4910334db1d698dd177f1bd23340ca0ea46203",
                "lastUpdateTime": "2026-10-07T10:33:52Z",
                "revision": "sha256:e7dc68a4dec90a35c2c6d8cdfedb7eaaee17fde45dced5898289df85069ec089", "size": 81804},
            "conditions": [{"lastTransitionTime": "2026-10-07T10:33:52Z",
                "message": "stored artifact: revision 'sha256:e7dc68a4dec90a35c2c6d8cdfedb7eaaee17fde45dced5898289df85069ec089'",
                "observedGeneration": 1, "reason": "Succeeded", "status": "True", "type": "Ready"}],
            "observedGeneration": 1
        }
    })
}

pub fn helm_chart() -> Value {
    json!({
        "apiVersion": "source.toolkit.fluxcd.io/v1",
        "kind": "HelmChart",
        "metadata": {"name": "flux-demo-podinfo-helm", "namespace": "flux-demo", "generation": 1},
        "spec": {"chart": "podinfo", "interval": "10m", "reconcileStrategy": "ChartVersion",
            "sourceRef": {"kind": "HelmRepository", "name": "podinfo"}, "version": "6.x"},
        "status": {
            "artifact": {"digest": "sha256:0d4f", "lastUpdateTime": "2026-10-07T10:33:52Z", "revision": "6.15.0"},
            "conditions": [{"type": "Ready", "status": "True", "reason": "ChartPullSucceeded",
                "message": "pulled 'podinfo' chart with version '6.15.0'"}],
            "observedChartName": "podinfo", "observedGeneration": 1
        }
    })
}

/// `type: oci` on Flux 2.9 (kind): source-controller left the status empty.
pub fn helm_repository_oci() -> Value {
    json!({
        "apiVersion": "source.toolkit.fluxcd.io/v1",
        "kind": "HelmRepository",
        "metadata": {"name": "podinfo-oci", "namespace": "flux-demo", "generation": 1,
            "creationTimestamp": "2026-10-07T10:00:00Z"},
        "spec": {"interval": "30m", "provider": "generic", "type": "oci",
            "url": "oci://ghcr.io/stefanprodan/charts"},
        "status": {}
    })
}

pub fn bucket() -> Value {
    json!({
        "apiVersion": "source.toolkit.fluxcd.io/v1",
        "kind": "Bucket",
        "metadata": {"name": "manifests", "namespace": "flux-demo", "generation": 2},
        "spec": {"bucketName": "manifests", "endpoint": "minio.flux-demo.svc:9000", "insecure": true,
            "interval": "5m", "provider": "generic", "secretRef": {"name": "minio-auth"}},
        "status": {
            "artifact": {"digest": "sha256:7c2bd8f1b5ea1ab4d4e7f0b07cd9f5a3c2b3b0c1d2e3f4a5b6c7d8e9f0a1b2c3",
                "lastUpdateTime": "2026-10-07T10:40:00Z", "path": "bucket/flux-demo/manifests/7c2bd8f1.tar.gz",
                "revision": "sha256:7c2bd8f1b5ea1ab4d4e7f0b07cd9f5a3c2b3b0c1d2e3f4a5b6c7d8e9f0a1b2c3", "size": 1024},
            "conditions": [{"lastTransitionTime": "2026-10-07T10:40:00Z", "observedGeneration": 2,
                "message": "stored artifact: revision 'sha256:7c2bd8f1'", "reason": "Succeeded",
                "status": "True", "type": "Ready"}],
            "observedGeneration": 2
        }
    })
}

/// Produced by another controller (source-watcher): reconciled by it, not on request.
pub fn external_artifact() -> Value {
    json!({
        "apiVersion": "source.toolkit.fluxcd.io/v1",
        "kind": "ExternalArtifact",
        "metadata": {"name": "composed", "namespace": "flux-demo", "generation": 1},
        "spec": {"sourceRef": {"apiVersion": "source.extensions.fluxcd.io/v1beta1",
            "kind": "ArtifactGenerator", "name": "composed"}},
        "status": {
            "artifact": {"digest": "sha256:aa11", "lastUpdateTime": "2026-10-07T10:41:00Z",
                "path": "externalartifact/flux-demo/composed/aa11.tar.gz",
                "revision": "latest@sha256:aa11bb22cc33dd44ee55ff6677889900aabbccddeeff00112233445566778899"},
            "conditions": [{"type": "Ready", "status": "True", "reason": "Succeeded",
                "message": "artifact is ready"}]
        }
    })
}

/// `v1beta2` (Flux 2.0/2.1): Alerts still had a status.
pub fn alert_v1beta2() -> Value {
    json!({
        "apiVersion": "notification.toolkit.fluxcd.io/v1beta2",
        "kind": "Alert",
        "metadata": {"name": "legacy", "namespace": "flux-system", "generation": 1},
        "spec": {"eventSeverity": "info", "eventSources": [{"kind": "Kustomization", "name": "*"}],
            "providerRef": {"name": "slack"}},
        "status": {
            "conditions": [{"type": "Ready", "status": "False", "reason": "ProviderNotFound",
                "message": "failed to get provider slack: Provider.notification.toolkit.fluxcd.io \"slack\" not found"}],
            "observedGeneration": 1
        }
    })
}

pub fn image_repository() -> Value {
    json!({
        "apiVersion": "image.toolkit.fluxcd.io/v1",
        "kind": "ImageRepository",
        "metadata": {"name": "podinfo", "namespace": "flux-demo", "generation": 1},
        "spec": {"image": "ghcr.io/stefanprodan/podinfo", "interval": "1h", "provider": "generic"},
        "status": {
            "canonicalImageName": "ghcr.io/stefanprodan/podinfo",
            "conditions": [{"lastTransitionTime": "2026-10-07T10:33:52Z",
                "message": "successful scan: found 97 tags with checksum 3282451018", "observedGeneration": 1,
                "reason": "Succeeded", "status": "True", "type": "Ready"}],
            "lastScanResult": {"revision": "3282451018", "scanTime": "2026-10-07T10:33:52Z", "tagCount": 97},
            "observedGeneration": 1
        }
    })
}

pub fn image_policy() -> Value {
    json!({
        "apiVersion": "image.toolkit.fluxcd.io/v1",
        "kind": "ImagePolicy",
        "metadata": {"name": "podinfo", "namespace": "flux-demo", "generation": 1},
        "spec": {"imageRepositoryRef": {"name": "podinfo"}, "policy": {"semver": {"range": "6.x"}}},
        "status": {
            "conditions": [{"lastTransitionTime": "2026-10-07T10:33:57Z",
                "message": "Latest image tag for ghcr.io/stefanprodan/podinfo resolved to 6.15.0",
                "observedGeneration": 1, "reason": "Succeeded", "status": "True", "type": "Ready"}],
            "latestRef": {"name": "ghcr.io/stefanprodan/podinfo", "tag": "6.15.0"},
            "observedGeneration": 1
        }
    })
}

/// `v1beta2`: `latestImage` as one string.
pub fn image_policy_v1beta2() -> Value {
    json!({
        "apiVersion": "image.toolkit.fluxcd.io/v1beta2",
        "kind": "ImagePolicy",
        "metadata": {"name": "podinfo", "namespace": "flux-system"},
        "spec": {"imageRepositoryRef": {"name": "podinfo"}, "policy": {"semver": {"range": "6.5.x"}}},
        "status": {
            "conditions": [{"type": "Ready", "status": "True", "reason": "Succeeded", "message": "ok"}],
            "latestImage": "ghcr.io/stefanprodan/podinfo:6.5.0"
        }
    })
}

pub fn image_update_automation() -> Value {
    json!({
        "apiVersion": "image.toolkit.fluxcd.io/v1",
        "kind": "ImageUpdateAutomation",
        "metadata": {"name": "podinfo", "namespace": "flux-demo", "generation": 1},
        "spec": {"interval": "30m", "sourceRef": {"kind": "GitRepository", "name": "podinfo"},
            "git": {"checkout": {"ref": {"branch": "main"}}, "push": {"branch": "main"},
                "commit": {"author": {"name": "fluxcdbot", "email": "fluxcdbot@users.noreply.github.com"}}},
            "update": {"path": "./clusters/dev", "strategy": "Setters"}},
        "status": {
            "conditions": [{"type": "Ready", "status": "True", "reason": "Succeeded",
                "message": "repository up-to-date"}],
            "lastAutomationRunTime": "2026-10-07T10:40:00Z",
            "lastPushCommit": "b7c1d2e3f4a5b6c7d8e9f00112233445566778899",
            "lastPushTime": "2026-10-06T18:00:00Z",
            "observedGeneration": 1
        }
    })
}

pub fn alert() -> Value {
    json!({
        "apiVersion": "notification.toolkit.fluxcd.io/v1beta3",
        "kind": "Alert",
        "metadata": {"name": "all-errors", "namespace": "flux-demo", "generation": 1},
        "spec": {"eventSeverity": "error",
            "eventSources": [{"kind": "Kustomization", "name": "*"}, {"kind": "HelmRelease", "name": "*"}],
            "providerRef": {"name": "webhook"}}
    })
}

pub fn provider() -> Value {
    json!({
        "apiVersion": "notification.toolkit.fluxcd.io/v1beta3",
        "kind": "Provider",
        "metadata": {"name": "webhook", "namespace": "flux-demo", "generation": 1},
        "spec": {"address": "http://user:pass@alerts.flux-demo.invalid/hook?token=abc", "type": "generic",
            "secretRef": {"name": "webhook-address"}}
    })
}

pub fn receiver() -> Value {
    json!({
        "apiVersion": "notification.toolkit.fluxcd.io/v1",
        "kind": "Receiver",
        "metadata": {"name": "github", "namespace": "flux-demo", "generation": 1},
        "spec": {"events": ["ping", "push"], "interval": "10m",
            "resources": [{"kind": "GitRepository", "name": "podinfo"}],
            "secretRef": {"name": "webhook-token"}, "type": "github"},
        "status": {
            "conditions": [{"lastTransitionTime": "2026-10-07T10:33:52Z",
                "message": "Receiver initialized for path: /hook/0543b6f51d7f604c2710f1052f5173a9655409a7ab93896a48dcc3735b541c35",
                "observedGeneration": 1, "reason": "Succeeded", "status": "True", "type": "Ready"}],
            "observedGeneration": 1,
            "webhookPath": "/hook/0543b6f51d7f604c2710f1052f5173a9655409a7ab93896a48dcc3735b541c35"
        }
    })
}

/// A Deployment kustomize-controller applied.
pub fn managed_deployment() -> Value {
    json!({
        "apiVersion": "apps/v1",
        "kind": "Deployment",
        "metadata": {"name": "podinfo", "namespace": "flux-podinfo",
            "labels": {"kustomize.toolkit.fluxcd.io/name": "podinfo",
                "kustomize.toolkit.fluxcd.io/namespace": "flux-demo"}}
    })
}

/// A Deployment helm-controller installed.
pub fn helm_deployment() -> Value {
    json!({
        "apiVersion": "apps/v1",
        "kind": "Deployment",
        "metadata": {"name": "podinfo-helm", "namespace": "flux-demo",
            "labels": {"helm.toolkit.fluxcd.io/name": "podinfo-helm",
                "helm.toolkit.fluxcd.io/namespace": "flux-demo",
                "app.kubernetes.io/managed-by": "Helm"}}
    })
}
