//! The `"files"` section of settings.json.

use kubyl_settings::SettingsSection;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct FilesSettings {
    /// Transfers that run at the same time per pod.
    pub concurrency_per_pod: usize,
    /// Compare SHA-256 on both sides after a transfer.
    pub verify: bool,
    /// Files larger than this download in resumable chunks of this size (MiB).
    pub chunk_size_mb: u64,
    /// Show dot files.
    pub show_hidden: bool,
    /// Extra folders in the local pane's bookmarks (`~/work/kube`).
    pub bookmarks: Vec<String>,
    /// Image of the debug container used to browse distroless containers (needs a shell and
    /// tar; busybox has both).
    pub debug_image: String,
    /// Bytes shown in a text preview (KiB).
    pub preview_limit_kb: u64,
}

impl Default for FilesSettings {
    fn default() -> Self {
        Self {
            concurrency_per_pod: 2,
            verify: true,
            chunk_size_mb: 4,
            show_hidden: false,
            bookmarks: Vec::new(),
            debug_image: "busybox:1.37".into(),
            preview_limit_kb: 256,
        }
    }
}

impl SettingsSection for FilesSettings {
    const KEY: Option<&'static str> = Some("files");
}
