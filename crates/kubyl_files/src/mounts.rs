//! Where a container's volumes are mounted (from the pod spec), so the browser can mark mount
//! points and refuse writes that can't work: ConfigMap, Secret, projected and downward-API
//! volumes are always read-only, and any mount can be `readOnly: true`.

use k8s_openapi::api::core::v1::{Pod, Volume};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MountSource {
    ConfigMap,
    Secret,
    Projected,
    DownwardApi,
    Pvc,
    EmptyDir,
    HostPath,
    Other,
}

impl MountSource {
    pub fn label(self) -> &'static str {
        match self {
            MountSource::ConfigMap => "ConfigMap",
            MountSource::Secret => "Secret",
            MountSource::Projected => "Projected",
            MountSource::DownwardApi => "Downward API",
            MountSource::Pvc => "PVC",
            MountSource::EmptyDir => "emptyDir",
            MountSource::HostPath => "hostPath",
            MountSource::Other => "Volume",
        }
    }

    /// The kubelet mounts these read-only; writes fail and would be lost anyway.
    pub fn always_read_only(self) -> bool {
        matches!(
            self,
            MountSource::ConfigMap
                | MountSource::Secret
                | MountSource::Projected
                | MountSource::DownwardApi
        )
    }
}

/// One volume mount of the container.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Mount {
    pub path: String,
    pub source: MountSource,
    /// The volume's object (ConfigMap/Secret/claim name).
    pub object: Option<String>,
    pub read_only: bool,
}

impl Mount {
    pub fn writable(&self) -> bool {
        !self.read_only && !self.source.always_read_only()
    }

    /// `Secret · read-only`, `ConfigMap`, `PVC data`.
    pub fn label(&self) -> String {
        let mut label = self.source.label().to_string();
        if !self.writable() {
            label.push_str(" · read-only");
        }
        label
    }
}

fn source(volume: &Volume) -> (MountSource, Option<String>) {
    if let Some(cm) = &volume.config_map {
        return (MountSource::ConfigMap, Some(cm.name.clone()));
    }
    if let Some(secret) = &volume.secret {
        return (MountSource::Secret, secret.secret_name.clone());
    }
    if volume.projected.is_some() {
        return (MountSource::Projected, None);
    }
    if volume.downward_api.is_some() {
        return (MountSource::DownwardApi, None);
    }
    if let Some(claim) = &volume.persistent_volume_claim {
        return (MountSource::Pvc, Some(claim.claim_name.clone()));
    }
    if volume.empty_dir.is_some() {
        return (MountSource::EmptyDir, None);
    }
    if let Some(host) = &volume.host_path {
        return (MountSource::HostPath, Some(host.path.clone()));
    }
    (MountSource::Other, None)
}

/// The mounts of `container` (regular, init or ephemeral), longest path first.
pub fn mounts(pod: &Pod, container: &str) -> Vec<Mount> {
    let Some(spec) = &pod.spec else {
        return Vec::new();
    };
    let volumes = spec.volumes.as_deref().unwrap_or_default();
    let volume_mounts = spec
        .containers
        .iter()
        .chain(spec.init_containers.iter().flatten())
        .find(|c| c.name == container)
        .and_then(|c| c.volume_mounts.clone())
        .or_else(|| {
            spec.ephemeral_containers
                .iter()
                .flatten()
                .find(|c| c.name == container)
                .and_then(|c| c.volume_mounts.clone())
        })
        .unwrap_or_default();
    let mut mounts: Vec<Mount> = volume_mounts
        .iter()
        .map(|m| {
            let (source, object) = volumes
                .iter()
                .find(|v| v.name == m.name)
                .map(source)
                .unwrap_or((MountSource::Other, None));
            Mount {
                path: m.mount_path.trim_end_matches('/').to_string(),
                source,
                object,
                read_only: m.read_only.unwrap_or(false),
            }
        })
        .collect();
    mounts.sort_by_key(|m| std::cmp::Reverse(m.path.len()));
    mounts
}

/// The mount a path lives in (the deepest one), if any.
pub fn mount_for<'a>(mounts: &'a [Mount], path: &str) -> Option<&'a Mount> {
    mounts.iter().find(|m| {
        path == m.path || (path.starts_with(&m.path) && path[m.path.len()..].starts_with('/'))
    })
}

/// The mount whose mount point is exactly `path`.
pub fn mount_at<'a>(mounts: &'a [Mount], path: &str) -> Option<&'a Mount> {
    mounts.iter().find(|m| m.path == path)
}

/// Why writing into `dir` is refused, if it is.
pub fn write_blocked(mounts: &[Mount], dir: &str) -> Option<String> {
    let mount = mount_for(mounts, dir)?;
    if mount.writable() {
        return None;
    }
    Some(if mount.source.always_read_only() {
        format!(
            "read-only mount ({}{}): change the {} instead",
            mount.source.label(),
            mount
                .object
                .as_ref()
                .map(|o| format!(" {o}"))
                .unwrap_or_default(),
            mount.source.label()
        )
    } else {
        format!("read-only mount ({})", mount.source.label())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pod() -> Pod {
        serde_json::from_value(serde_json::json!({
            "spec": {
                "containers": [{
                    "name": "api",
                    "volumeMounts": [
                        {"name": "config", "mountPath": "/app/config"},
                        {"name": "creds", "mountPath": "/app/config/secrets"},
                        {"name": "data", "mountPath": "/data"},
                        {"name": "cache", "mountPath": "/cache", "readOnly": true}
                    ]
                }],
                "volumes": [
                    {"name": "config", "configMap": {"name": "checkout-config"}},
                    {"name": "creds", "secret": {"secretName": "checkout-db"}},
                    {"name": "data", "persistentVolumeClaim": {"claimName": "data-0"}},
                    {"name": "cache", "emptyDir": {}}
                ]
            }
        }))
        .unwrap()
    }

    #[test]
    fn finds_the_deepest_mount() {
        let mounts = mounts(&pod(), "api");
        assert_eq!(mounts.len(), 4);
        let secret = mount_for(&mounts, "/app/config/secrets/password").unwrap();
        assert_eq!(secret.source, MountSource::Secret);
        assert_eq!(
            mount_for(&mounts, "/app/config/application.yaml")
                .unwrap()
                .source,
            MountSource::ConfigMap
        );
        assert!(mount_for(&mounts, "/app/configuration").is_none());
        assert_eq!(
            mount_at(&mounts, "/data").unwrap().object.as_deref(),
            Some("data-0")
        );
    }

    #[test]
    fn blocks_writes_to_read_only_mounts() {
        let mounts = mounts(&pod(), "api");
        let reason = write_blocked(&mounts, "/app/config/secrets").unwrap();
        assert!(
            reason.starts_with("read-only mount (Secret checkout-db)"),
            "{reason}"
        );
        assert!(write_blocked(&mounts, "/app/config").is_some());
        assert_eq!(
            write_blocked(&mounts, "/cache/x").as_deref(),
            Some("read-only mount (emptyDir)")
        );
        assert!(write_blocked(&mounts, "/data/dumps").is_none());
        assert!(write_blocked(&mounts, "/tmp").is_none());
        assert_eq!(
            mount_at(&mounts, "/app/config/secrets").unwrap().label(),
            "Secret · read-only"
        );
    }
}
