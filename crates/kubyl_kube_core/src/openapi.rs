//! OpenAPI v3 schemas (`/openapi/v3`), cached on disk.
//!
//! The index maps each group-version to a URL carrying a content hash
//! (`/openapi/v3/apis/apps/v1?hash=…`); each spec response has an ETag. Specs are stored under
//! `<cache dir>/kubyl/openapi/<cluster>/` with their ETag and revalidated with `If-None-Match`,
//! so an unchanged schema costs one 304. Phases 04 (YAML validation) and 08 (operators) use it.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use http::{Request, StatusCode, header};
use http_body_util::BodyExt as _;
use kube::Client;
use serde::Deserialize;

/// The OpenAPI v3 discovery document: group-version path → spec URL.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
pub struct OpenApiIndex {
    pub paths: BTreeMap<String, OpenApiPath>,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct OpenApiPath {
    #[serde(rename = "serverRelativeURL")]
    pub server_relative_url: String,
}

impl OpenApiIndex {
    /// The index key for a group-version: `api/v1` or `apis/apps/v1`.
    pub fn key(group: &str, version: &str) -> String {
        if group.is_empty() {
            format!("api/{version}")
        } else {
            format!("apis/{group}/{version}")
        }
    }
}

/// Where a cluster's specs are cached: `<cache dir>/kubyl/openapi/<hash of cluster key>`.
pub fn cache_dir(cluster_key: &str) -> PathBuf {
    use std::hash::{Hash as _, Hasher as _};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    cluster_key.hash(&mut hasher);
    dirs::cache_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join("kubyl")
        .join("openapi")
        .join(format!("{:016x}", hasher.finish()))
}

fn cache_file(dir: &Path, path: &str) -> PathBuf {
    let name: String = path
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '.' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect();
    dir.join(format!("{name}.json"))
}

/// GETs `url`, revalidating the cached copy in `dir`. Returns the body.
async fn fetch_cached(client: &Client, dir: &Path, url: &str) -> Result<Vec<u8>, kube::Error> {
    let file = cache_file(dir, url.split('?').next().unwrap_or(url));
    let etag_file = file.with_extension("etag");
    let cached_etag = tokio::fs::read_to_string(&etag_file).await.ok();
    let mut request = Request::get(url).header(header::ACCEPT, "application/json");
    if let Some(etag) = &cached_etag {
        request = request.header(header::IF_NONE_MATCH, etag.trim());
    }
    let request = request.body(Vec::new()).map_err(kube::Error::HttpError)?;
    let response = client.send(request.map(Into::into)).await?;
    let status = response.status();
    if status == StatusCode::NOT_MODIFIED
        && let Ok(bytes) = tokio::fs::read(&file).await
    {
        return Ok(bytes);
    }
    let etag = response
        .headers()
        .get(header::ETAG)
        .and_then(|v| v.to_str().ok())
        .map(String::from);
    let bytes = response
        .into_body()
        .collect()
        .await
        .map_err(|err| kube::Error::Service(Box::new(err)))?
        .to_bytes()
        .to_vec();
    if !status.is_success() {
        return Err(kube::Error::Api(
            kube::core::Status::failure(&String::from_utf8_lossy(&bytes), "OpenAPI")
                .with_code(status.as_u16())
                .boxed(),
        ));
    }
    if tokio::fs::create_dir_all(dir).await.is_ok() {
        tokio::fs::write(&file, &bytes).await.ok();
        match etag {
            Some(etag) => tokio::fs::write(&etag_file, etag).await.ok(),
            None => tokio::fs::remove_file(&etag_file).await.ok(),
        };
    }
    Ok(bytes)
}

/// The OpenAPI v3 index. Errors with 404 on servers older than 1.24.
pub async fn index(client: &Client, dir: &Path) -> Result<OpenApiIndex, kube::Error> {
    let bytes = fetch_cached(client, dir, "/openapi/v3").await?;
    serde_json::from_slice(&bytes).map_err(kube::Error::SerdeError)
}

/// The spec of one group-version (`apis/apps/v1`), as JSON.
pub async fn spec(
    client: &Client,
    dir: &Path,
    path: &str,
) -> Result<serde_json::Value, kube::Error> {
    let index = index(client, dir).await?;
    let url = index
        .paths
        .get(path)
        .map(|p| p.server_relative_url.clone())
        .unwrap_or_else(|| format!("/openapi/v3/{path}"));
    let bytes = fetch_cached(client, dir, &url).await?;
    serde_json::from_slice(&bytes).map_err(kube::Error::SerdeError)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn index_keys_and_cache_files() {
        assert_eq!(OpenApiIndex::key("", "v1"), "api/v1");
        assert_eq!(OpenApiIndex::key("apps", "v1"), "apis/apps/v1");
        let index: OpenApiIndex = serde_json::from_str(
            r#"{"paths":{"apis/apps/v1":{"serverRelativeURL":"/openapi/v3/apis/apps/v1?hash=AB"}}}"#,
        )
        .unwrap();
        assert!(
            index.paths["apis/apps/v1"]
                .server_relative_url
                .ends_with("hash=AB")
        );
        assert_eq!(
            cache_file(Path::new("/c"), "/openapi/v3/apis/apps/v1"),
            PathBuf::from("/c/_openapi_v3_apis_apps_v1.json")
        );
        assert_ne!(cache_dir("a"), cache_dir("b"));
    }
}
