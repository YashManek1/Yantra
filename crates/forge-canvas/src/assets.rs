//! # forge-canvas::assets: Asset Download Pipeline
//!
//! Downloads images and fonts referenced in a cloned HTML document into a
//! `public/` sub-directory of the emitted project, and rewrites `src`/`href`
//! references so the React project serves them locally.
//!
//! ## Input
//! - `DomTree` — parsed document whose nodes carry `src`/`href` attributes
//! - `base_url: &url::Url` — origin of the cloned site for resolving relative URLs
//! - `project_root: &Path` — emitted project directory to write `public/` into
//! - `http_client: &reqwest::Client` — shared HTTP client for all downloads
//!
//! ## Output
//! - Files written to `<project_root>/public/<filename>`
//! - `AssetManifest` — map of original URL → local `public/<filename>` path
//!
//! ## Related
//! - `forge-canvas::clone` — calls `download_assets` after HTML parsing
//! - `forge-canvas::tsx_writer` — uses the manifest to rewrite `src` attributes

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use url::Url;

use crate::dom::DomTree;
use crate::error::{CanvasError, CanvasResult};

/// Maps an original asset URL to its local path under `public/`.
pub type AssetManifest = HashMap<String, PathBuf>;

/// Downloads all image and font assets referenced in `dom_tree` and writes
/// them into `<project_root>/public/`. Returns a manifest mapping original
/// URLs to local file paths.
///
/// Assets that cannot be fetched are skipped with a warning; only the initial
/// `public/` directory creation can produce a hard error.
///
/// # Errors
///
/// Returns `CanvasError::AssetDownloadFailed` when the `public/` directory
/// cannot be created.
pub async fn download_assets(
    dom_tree: &DomTree,
    base_url: &Url,
    project_root: &Path,
    http_client: &reqwest::Client,
) -> CanvasResult<AssetManifest> {
    let public_directory = project_root.join("public");
    std::fs::create_dir_all(&public_directory).map_err(|source| {
        CanvasError::AssetDownloadFailed(format!("cannot create public/ directory: {source}"))
    })?;

    let asset_urls = collect_asset_urls(dom_tree, base_url);
    let mut asset_manifest: AssetManifest = HashMap::new();

    for original_url in asset_urls {
        match fetch_and_save_asset(&original_url, &public_directory, http_client).await {
            Ok(local_path) => {
                tracing::info!("downloaded asset {} → {:?}", original_url, local_path);
                asset_manifest.insert(original_url, local_path);
            }
            Err(download_error) => {
                tracing::warn!("skipping asset {}: {}", original_url, download_error);
            }
        }
    }

    Ok(asset_manifest)
}

/// Collects all image and font asset URLs from the DOM tree nodes.
///
/// Filters out data URIs and javascript: pseudo-URLs. Relative URLs are
/// resolved against `base_url`. Duplicate resolved URLs are deduplicated.
fn collect_asset_urls(dom_tree: &DomTree, base_url: &Url) -> Vec<String> {
    let mut collected_urls: Vec<String> = Vec::new();
    for dom_node in dom_tree.nodes.values() {
        let attribute_url = match dom_node.tag.as_str() {
            "img" | "source" => dom_node.attributes.get("src"),
            "link" => dom_node.attributes.get("href"),
            _ => None,
        };
        if let Some(raw_url) = attribute_url {
            if raw_url.starts_with("data:") || raw_url.starts_with("javascript:") {
                continue;
            }
            if let Ok(resolved_url) = base_url.join(raw_url) {
                let resolved_url_string = resolved_url.to_string();
                if !collected_urls.contains(&resolved_url_string) {
                    collected_urls.push(resolved_url_string);
                }
            }
        }
    }
    collected_urls
}

/// Fetches a single asset URL and writes its bytes to `public_directory`.
///
/// Returns the local `PathBuf` on success.
///
/// # Errors
///
/// Returns `CanvasError::AssetDownloadFailed` on network error, non-2xx
/// response, body read failure, or file write failure.
async fn fetch_and_save_asset(
    asset_url: &str,
    public_directory: &Path,
    http_client: &reqwest::Client,
) -> CanvasResult<PathBuf> {
    let response = http_client
        .get(asset_url)
        .send()
        .await
        .map_err(|network_error| {
            CanvasError::AssetDownloadFailed(format!("GET {asset_url}: {network_error}"))
        })?;

    if !response.status().is_success() {
        return Err(CanvasError::AssetDownloadFailed(format!(
            "GET {asset_url}: HTTP {}",
            response.status()
        )));
    }

    let filename = derive_asset_filename(asset_url);
    let local_path = public_directory.join(&filename);

    let asset_bytes = response.bytes().await.map_err(|read_error| {
        CanvasError::AssetDownloadFailed(format!("reading body for {asset_url}: {read_error}"))
    })?;

    std::fs::write(&local_path, &asset_bytes).map_err(|write_error| {
        CanvasError::AssetDownloadFailed(format!("writing {local_path:?}: {write_error}"))
    })?;

    Ok(local_path)
}

/// Derives a local filename from an asset URL by taking the last path segment
/// and stripping any query string.
///
/// Falls back to `"asset"` for URLs with no identifiable filename segment.
fn derive_asset_filename(asset_url: &str) -> String {
    asset_url
        .rsplit('/')
        .next()
        .and_then(|segment| {
            let clean_segment = segment.split('?').next().unwrap_or(segment);
            if clean_segment.is_empty() {
                None
            } else {
                Some(clean_segment.to_owned())
            }
        })
        .unwrap_or_else(|| "asset".to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn derive_asset_filename_from_url_path() {
        assert_eq!(
            derive_asset_filename("https://example.com/images/logo.png"),
            "logo.png"
        );
        assert_eq!(
            derive_asset_filename("https://cdn.example.com/font.woff2?v=2"),
            "font.woff2"
        );
        assert_eq!(derive_asset_filename("https://example.com/"), "asset");
    }

    #[test]
    fn collect_asset_urls_filters_data_uris() {
        use std::collections::HashMap;

        use crate::dom::{DomNode, DomTree, YantraId};

        let image_id = YantraId::from_path("body>img(0)");
        let mut image_node = DomNode {
            yantra_id: image_id.clone(),
            tag: "img".to_owned(),
            classes: Vec::new(),
            attributes: HashMap::new(),
            inline_style: HashMap::new(),
            text_content: String::new(),
            children: Vec::new(),
        };
        image_node
            .attributes
            .insert("src".to_owned(), "data:image/png;base64,abc".to_owned());

        let mut nodes = HashMap::new();
        nodes.insert(image_id.clone(), image_node);

        let dom_tree = DomTree {
            root_id: image_id,
            nodes,
        };

        let base_url = Url::parse("https://example.com").unwrap();
        let found_urls = collect_asset_urls(&dom_tree, &base_url);
        assert!(found_urls.is_empty(), "data: URIs must be filtered out");
    }

    #[test]
    fn collect_asset_urls_resolves_relative_hrefs() {
        use std::collections::HashMap;

        use crate::dom::{DomNode, DomTree, YantraId};

        let link_id = YantraId::from_path("head>link(0)");
        let mut link_node = DomNode {
            yantra_id: link_id.clone(),
            tag: "link".to_owned(),
            classes: Vec::new(),
            attributes: HashMap::new(),
            inline_style: HashMap::new(),
            text_content: String::new(),
            children: Vec::new(),
        };
        link_node
            .attributes
            .insert("href".to_owned(), "/fonts/roboto.woff2".to_owned());

        let mut nodes = HashMap::new();
        nodes.insert(link_id.clone(), link_node);

        let dom_tree = DomTree {
            root_id: link_id,
            nodes,
        };

        let base_url = Url::parse("https://example.com").unwrap();
        let found_urls = collect_asset_urls(&dom_tree, &base_url);
        assert_eq!(found_urls.len(), 1);
        assert_eq!(found_urls[0], "https://example.com/fonts/roboto.woff2");
    }
}
