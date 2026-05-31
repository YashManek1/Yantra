//! # forge-canvas::clone: URL → ClonedSite Fetcher + Multi-Page Crawler
//!
//! Fetches a URL with `reqwest` and parses the HTML with `scraper`. External
//! stylesheets referenced by `<link rel="stylesheet">` are fetched
//! concurrently and returned alongside any inline `<style>` blocks.
//!
//! Provides `crawl_site` for bounded same-origin BFS crawling, and —
//! when the `render-js` feature is enabled — `render_js_html` for
//! headless Chromium rendering of JavaScript-heavy pages.
//!
//! ## Input
//! - `&url::Url` — fully qualified target URL (for `clone_url`)
//! - `&str` + `usize` — start URL and page cap (for `crawl_site`)
//!
//! ## Output
//! - `ClonedSite { html, base_url, inline_styles, external_styles }`
//! - `Vec<ClonedSite>` from `crawl_site`
//!
//! ## Related
//! - `forge-canvas::dom::DomTree::from_html` — primary downstream consumer
//! - `forge-canvas::tsx_writer::emit_project` — uses the CSS payload
//! - `forge-canvas::assets::download_assets` — downloads assets after cloning

use futures_util::stream::StreamExt;
use reqwest::Client;
use scraper::{Html, Selector};
use url::Url;

use crate::error::{CanvasError, CanvasResult};

/// HTML + cascaded CSS captured from a remote URL.
#[derive(Debug, Clone)]
pub struct ClonedSite {
    /// Raw HTML text as fetched from the URL.
    pub html: String,
    /// The resolved base URL used for relative-href resolution.
    pub base_url: Url,
    /// Inline `<style>...</style>` blocks from the document, in order.
    pub inline_styles: Vec<String>,
    /// External stylesheets fetched from `<link rel="stylesheet">` hrefs.
    pub external_styles: Vec<String>,
}

const USER_AGENT: &str = "Mozilla/5.0 (Yantra-Canvas) yantra/0.1";
const REQUEST_TIMEOUT_SECS: u64 = 10;
const MAX_CONCURRENT_STYLE_FETCHES: usize = 8;

/// Fetches `target_url`, parses the document, and pulls all referenced CSS.
///
/// # Errors
///
/// Returns `CanvasError::CloneFailed` on any HTTP error, non-2xx response,
/// invalid UTF-8 body, or HTML selector construction failure.
pub async fn clone_url(target_url: &Url) -> CanvasResult<ClonedSite> {
    let http_client = Client::builder()
        .user_agent(USER_AGENT)
        .timeout(std::time::Duration::from_secs(REQUEST_TIMEOUT_SECS))
        .build()
        .map_err(|err| CanvasError::CloneFailed {
            url: target_url.to_string(),
            reason: format!("client build failed: {err}"),
        })?;

    let http_response = http_client
        .get(target_url.as_str())
        .send()
        .await
        .map_err(|err| CanvasError::CloneFailed {
            url: target_url.to_string(),
            reason: format!("request failed: {err}"),
        })?;

    if !http_response.status().is_success() {
        return Err(CanvasError::CloneFailed {
            url: target_url.to_string(),
            reason: format!("HTTP {}", http_response.status()),
        });
    }

    let resolved_base_url = http_response.url().clone();
    let raw_html_text = http_response
        .text()
        .await
        .map_err(|err| CanvasError::CloneFailed {
            url: target_url.to_string(),
            reason: format!("body read failed: {err}"),
        })?;

    let parsed_html = Html::parse_document(&raw_html_text);

    let style_selector = Selector::parse("style").map_err(|err| CanvasError::CloneFailed {
        url: target_url.to_string(),
        reason: format!("style selector: {err}"),
    })?;
    let inline_styles: Vec<String> = parsed_html
        .select(&style_selector)
        .map(|style_element| style_element.text().collect::<String>())
        .filter(|content| !content.trim().is_empty())
        .collect();

    let link_selector =
        Selector::parse("link[rel='stylesheet']").map_err(|err| CanvasError::CloneFailed {
            url: target_url.to_string(),
            reason: format!("link selector: {err}"),
        })?;
    let external_hrefs: Vec<Url> = parsed_html
        .select(&link_selector)
        .filter_map(|link_element| link_element.value().attr("href"))
        .filter_map(|relative_href| resolved_base_url.join(relative_href).ok())
        .collect();

    let external_styles = fetch_external_styles(&http_client, &external_hrefs).await;

    Ok(ClonedSite {
        html: raw_html_text,
        base_url: resolved_base_url,
        inline_styles,
        external_styles,
    })
}

async fn fetch_external_styles(http_client: &Client, urls: &[Url]) -> Vec<String> {
    futures_util::stream::iter(urls.iter().cloned())
        .map(|stylesheet_url| {
            let client_clone = http_client.clone();
            async move {
                let request_result = client_clone.get(stylesheet_url.as_str()).send().await;
                let response = match request_result {
                    Ok(resp) if resp.status().is_success() => resp,
                    _ => return String::new(),
                };
                response.text().await.unwrap_or_default()
            }
        })
        .buffer_unordered(MAX_CONCURRENT_STYLE_FETCHES)
        .filter(|css_text| {
            let is_nonempty = !css_text.trim().is_empty();
            async move { is_nonempty }
        })
        .collect()
        .await
}

/// Crawls same-origin pages starting from `start_url`, bounded by `max_pages`.
///
/// Performs a BFS from `start_url`, following `<a href>` links that share the
/// same origin. Returns one `ClonedSite` per successfully fetched page.
///
/// # Errors
///
/// Returns `CanvasError::InvalidUrl` if `start_url` cannot be parsed.
pub async fn crawl_site(start_url: &str, max_pages: usize) -> CanvasResult<Vec<ClonedSite>> {
    use std::collections::{HashSet, VecDeque};

    let origin_url = Url::parse(start_url)
        .map_err(|parse_error| CanvasError::InvalidUrl(parse_error.to_string()))?;
    let origin_host = origin_url.host_str().unwrap_or("").to_owned();

    let mut visited_urls: HashSet<String> = HashSet::new();
    let mut crawl_queue: VecDeque<String> = VecDeque::new();
    let mut crawled_sites: Vec<ClonedSite> = Vec::new();

    crawl_queue.push_back(start_url.to_owned());
    visited_urls.insert(start_url.to_owned());

    while let Some(current_url_string) = crawl_queue.pop_front() {
        if crawled_sites.len() >= max_pages {
            break;
        }

        let current_url = match Url::parse(&current_url_string) {
            Ok(parsed_url) => parsed_url,
            Err(parse_error) => {
                tracing::warn!(
                    "skipping malformed URL {} during crawl: {}",
                    current_url_string,
                    parse_error
                );
                continue;
            }
        };

        let cloned_page = match clone_url(&current_url).await {
            Ok(cloned_site) => cloned_site,
            Err(fetch_error) => {
                tracing::warn!(
                    "skipping {} during crawl: {}",
                    current_url_string,
                    fetch_error
                );
                continue;
            }
        };

        let parsed_html = scraper::Html::parse_document(&cloned_page.html);
        let discovered_links = extract_same_origin_links(&parsed_html, &origin_url, &origin_host);
        for discovered_link in discovered_links {
            if !visited_urls.contains(&discovered_link) {
                visited_urls.insert(discovered_link.clone());
                crawl_queue.push_back(discovered_link);
            }
        }

        crawled_sites.push(cloned_page);
    }

    Ok(crawled_sites)
}

/// Extracts all `<a href>` links from a parsed document that share the same
/// origin as `origin_host`, resolved against `base_url`.
fn extract_same_origin_links(
    parsed_html: &scraper::Html,
    base_url: &Url,
    origin_host: &str,
) -> Vec<String> {
    let anchor_selector = match scraper::Selector::parse("a[href]") {
        Ok(selector) => selector,
        Err(_) => return Vec::new(),
    };

    let mut same_origin_links: Vec<String> = Vec::new();
    for anchor_element in parsed_html.select(&anchor_selector) {
        let href_value = match anchor_element.value().attr("href") {
            Some(href) => href,
            None => continue,
        };
        if href_value.starts_with('#') || href_value.starts_with("javascript:") {
            continue;
        }
        if let Ok(resolved_link) = base_url.join(href_value) {
            if resolved_link.host_str() == Some(origin_host) {
                same_origin_links.push(resolved_link.to_string());
            }
        }
    }
    same_origin_links
}

/// Renders a JavaScript-heavy page using a headless Chromium browser and
/// returns the post-JS-execution HTML.
///
/// Only compiled when the `render-js` feature is enabled.
///
/// # Errors
///
/// Returns `CanvasError::RenderJsFailed` if the browser cannot be launched
/// or the page fails to load within the timeout.
#[cfg(feature = "render-js")]
pub async fn render_js_html(target_url: &str) -> CanvasResult<String> {
    use std::time::Duration;

    use chromiumoxide::browser::BrowserConfig;
    use chromiumoxide::Browser;

    let browser_config = BrowserConfig::builder()
        .build()
        .map_err(|build_error| CanvasError::RenderJsFailed(build_error.to_string()))?;

    let (browser_instance, mut browser_handle) = Browser::launch(browser_config)
        .await
        .map_err(|launch_error| CanvasError::RenderJsFailed(launch_error.to_string()))?;

    let browser_page = browser_instance
        .new_page(target_url)
        .await
        .map_err(|page_error| CanvasError::RenderJsFailed(page_error.to_string()))?;

    tokio::time::sleep(Duration::from_millis(1500)).await;

    let rendered_html = browser_page
        .content()
        .await
        .map_err(|content_error| CanvasError::RenderJsFailed(content_error.to_string()))?;

    browser_handle.close().await.ok();

    Ok(rendered_html)
}

/// Attempts to render the page at `target_url` with headless Chromium if the
/// `render-js` feature is compiled in. Returns `Some(html)` on success, or
/// `None` when the feature is absent (after logging a warning).
///
/// This function always compiles and is the stable entry point for callers
/// that do not want to scatter `#[cfg(feature = "render-js")]` attributes.
///
/// # Errors
///
/// Propagates `CanvasError::RenderJsFailed` when the `render-js` feature is
/// enabled but the browser fails to launch or the page fails to load.
pub async fn try_render_js_html(target_url: &str) -> CanvasResult<Option<String>> {
    #[cfg(feature = "render-js")]
    {
        let rendered_html = render_js_html(target_url).await?;
        return Ok(Some(rendered_html));
    }

    #[cfg(not(feature = "render-js"))]
    {
        tracing::warn!(
            url = target_url,
            "--render-js requested but the `render-js` feature is not compiled in; \
             falling back to static HTTP fetch"
        );
        return Ok(None);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_inline_styles_from_local_html() {
        let html_text = r"
            <html>
              <head><style>body { color: red; }</style></head>
              <body><h1>hi</h1></body>
            </html>
        ";
        let parsed = Html::parse_document(html_text);
        let selector = Selector::parse("style").unwrap();
        let collected: Vec<String> = parsed
            .select(&selector)
            .map(|el| el.text().collect::<String>())
            .filter(|content| !content.trim().is_empty())
            .collect();
        assert_eq!(collected.len(), 1);
        assert!(collected[0].contains("color: red"));
    }
}
