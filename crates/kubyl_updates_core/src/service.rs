//! The update state of a cluster and building its provider: what `kubyl_updates::Updates` keeps
//! per cluster and the decisions it makes, without the UI. Cluster facts come from the
//! connection manager's core.

use std::sync::Arc;
use std::time::{Duration, Instant};

use kubyl_base::ClusterId;
use kubyl_kube_core::manager::ManagerCore;

use crate::check::Check;
use crate::detect::{self, Detected, Facts};
use crate::fallback::ReadOnly;
use crate::model::{Note, ProviderKind, Status};
use crate::provider::{ProviderError, UpdateProvider};
use crate::providers;
use crate::settings::UpdatesSettings;

/// Read interval while nothing runs.
pub const IDLE: Duration = Duration::from_secs(15);
/// Read interval while an update runs.
pub const UPDATING: Duration = Duration::from_secs(5);
/// How long a cluster stays after the last lease went.
pub const KEEP: Duration = Duration::from_secs(60);
/// How long a pre-flight run waits for the Helm releases to list.
pub const HELM_WAIT: Duration = Duration::from_secs(15);

/// A read's outcome.
#[derive(Clone, Debug)]
pub enum ReadState {
    Loading,
    Ready(Arc<Status>),
    Failed(ProviderError),
}

/// What a view shows for a cluster.
#[derive(Clone, Debug)]
pub enum UpdateState {
    NotConnected,
    /// Discovery hasn't finished: the provider isn't known yet.
    Detecting,
    Known {
        detected: Detected,
        read: ReadState,
        /// The last good read while a newer one failed.
        last: Option<Arc<Status>>,
        fetched_at: Option<Instant>,
    },
}

/// A pre-flight run for one target.
#[derive(Clone, Debug)]
pub struct Preflight {
    pub target: String,
    /// The version it ran against (a new version runs it again).
    pub current: String,
    pub started: Instant,
    pub finished: Option<Instant>,
    pub checks: Vec<Check>,
}

impl Preflight {
    pub fn running(&self) -> bool {
        self.finished.is_none()
    }
}

/// The facts detection looks at, if the cluster is connected and discovered.
pub fn facts(manager: &ManagerCore, cluster: &ClusterId) -> Option<Facts> {
    let connection = manager.cluster(cluster)?;
    let discovery = connection.discovery.as_ref()?;
    let context = manager.context(cluster);
    let exec_command = context.and_then(|c| match &c.auth {
        kubyl_kube_core::auth::AuthMethod::Exec(exec) => Some(exec.command.clone()),
        _ => None,
    });
    Some(
        Facts {
            git_version: connection
                .info
                .as_ref()
                .map(|i| i.version.clone())
                .unwrap_or_default(),
            server: context.and_then(|c| c.server.clone()).unwrap_or_default(),
            names: context
                .map(|c| format!("{} {}", c.context, c.cluster))
                .unwrap_or_default(),
            exec_command,
            ..Facts::default()
        }
        .with_discovery(discovery),
    )
}

/// Builds the provider of a detected kind.
pub fn build(
    manager: Option<&ManagerCore>,
    settings: &UpdatesSettings,
    cluster: &ClusterId,
    detected: &Detected,
    facts: &Facts,
    client: kube::Client,
) -> Arc<dyn UpdateProvider> {
    let display = manager
        .map(|m| m.display_name(cluster))
        .unwrap_or_else(|| cluster.to_string());
    let distribution = manager.and_then(|m| {
        m.cluster(cluster)
            .and_then(|c| c.info.as_ref())
            .map(|i| i.distribution)
    });
    let read_only = |label: &str, reason: String, notes: Vec<Note>| -> Arc<dyn UpdateProvider> {
        Arc::new(ReadOnly::new(
            client.clone(),
            detected.kind,
            label,
            reason,
            notes,
        ))
    };
    match detected.kind {
        ProviderKind::OpenShift => Arc::new(crate::openshift::OpenShift::new(client)),
        ProviderKind::K3s | ProviderKind::Rke2 => {
            if facts.suc_plans {
                Arc::new(crate::suc::Suc::new(client, detected.kind))
            } else {
                read_only(
                    detected.kind.label(),
                    "Kubyl updates k3s and RKE2 through system-upgrade-controller's Plans, which this cluster doesn't have.".into(),
                    vec![Note {
                        warning: false,
                        title: "system-upgrade-controller isn't installed".into(),
                        text: "Install it to update this cluster from Kubyl with Plans.".into(),
                        command: None,
                        url: Some(if detected.kind == ProviderKind::K3s {
                            "https://docs.k3s.io/upgrades/automated".into()
                        } else {
                            "https://docs.rke2.io/upgrades/automated_upgrade".into()
                        }),
                    }],
                )
            }
        }
        ProviderKind::ClusterApi => Arc::new(crate::capi::ClusterApi::new(
            client,
            capi_versions(manager, cluster),
        )),
        kind if kind.is_cloud() => {
            if let Some(provider) = cloud(kind, manager, settings, cluster, &display) {
                provider
            } else {
                let feature = providers::feature_of(kind).unwrap_or_default();
                read_only(
                    kind.label(),
                    format!("This build doesn't include the {} provider.", kind.label()),
                    vec![Note {
                        warning: true,
                        title: format!("This build doesn't include the {} provider", kind.label()),
                        text: format!(
                            "Kubyl was built without the {feature} feature: the version, nodes and pre-flight checks still work, updates don't."
                        ),
                        command: None,
                        url: None,
                    }],
                )
            }
        }
        _ => read_only(
            detect::self_managed_label(distribution, facts),
            "Kubyl doesn't know how this cluster was installed, so it can't update it. Update it with the tool that installed it.".into(),
            Vec::new(),
        ),
    }
}

/// The Cluster API versions the cluster serves (preferred).
pub fn capi_versions(
    manager: Option<&ManagerCore>,
    cluster: &ClusterId,
) -> crate::capi::CapiVersions {
    let discovery = manager.and_then(|m| m.discovery(cluster));
    let version = |group: &str, resource: &str| {
        discovery.as_ref().and_then(|d| {
            d.preferred()
                .find(|r| r.gvr.group == group && r.gvr.resource == resource)
                .map(|r| r.gvr.version.clone())
        })
    };
    let defaults = crate::capi::CapiVersions::default();
    crate::capi::CapiVersions {
        cluster: version("cluster.x-k8s.io", "clusters").unwrap_or(defaults.cluster),
        control_plane: version("controlplane.cluster.x-k8s.io", "kubeadmcontrolplanes"),
    }
}

/// The cloud provider, when this build has it.
#[allow(unused_variables)]
pub fn cloud(
    kind: ProviderKind,
    manager: Option<&ManagerCore>,
    settings: &UpdatesSettings,
    cluster: &ClusterId,
    display: &str,
) -> Option<Arc<dyn UpdateProvider>> {
    if !providers::cloud_built(kind) {
        return None;
    }
    let manager = manager?;
    let context = manager.context(cluster)?;
    let settings = settings.for_keys(&manager.settings_keys(cluster));
    let context = providers::CloudContext {
        display_name: display.to_string(),
        kubeconfig: context.file.clone(),
        context: context.context.clone(),
        user: context.user.clone(),
        cluster_entry: context.cluster.clone(),
        server: context.server.clone().unwrap_or_default(),
        settings,
    };
    match kind {
        #[cfg(feature = "updates-eks")]
        ProviderKind::Eks => Some(providers::eks::provider(context)),
        #[cfg(feature = "updates-gke")]
        ProviderKind::Gke => Some(providers::gke::provider(context)),
        #[cfg(feature = "updates-aks")]
        ProviderKind::Aks => Some(providers::aks::provider(context)),
        _ => {
            let _ = context;
            None
        }
    }
}

/// A cloud read that failed for want of credentials or the cloud API: the read-only view
/// takes over with a note.
pub fn cloud_unavailable(err: &ProviderError) -> bool {
    matches!(
        err,
        ProviderError::Credentials { .. }
            | ProviderError::Unavailable(_)
            | ProviderError::Forbidden { .. }
    )
}

pub type NoteFn = Box<dyn Fn(&ProviderError) -> Note + Send>;

/// For cloud providers: the read-only provider to fall back to and the note it gets.
pub fn fallback_for(
    manager: Option<&ManagerCore>,
    cluster: &ClusterId,
    provider: &Arc<dyn UpdateProvider>,
) -> Option<(ReadOnly, NoteFn)> {
    let kind = provider.kind();
    if !kind.is_cloud() {
        return None;
    }
    let client = manager?.client(cluster)?;
    let label = kind.label();
    let fallback = ReadOnly::new(
        client,
        kind,
        label,
        format!("The {label} API isn't available."),
        Vec::new(),
    );
    let note: NoteFn = Box::new(move |err: &ProviderError| {
        let (title, command) = match err {
            ProviderError::Credentials { command, .. } => (
                format!("{label} credentials aren't available"),
                command.clone(),
            ),
            ProviderError::Forbidden { .. } => (format!("The {label} API denied access"), None),
            _ => (format!("The {label} API isn't available"), None),
        };
        Note {
            warning: true,
            title,
            text: format!("{err} Kubernetes-side checks still run."),
            command,
            url: None,
        }
    });
    Some((fallback, note))
}
