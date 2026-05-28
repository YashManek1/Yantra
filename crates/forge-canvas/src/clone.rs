//! # forge-canvas::clone: URL → ClonedSite Fetcher
//!
//! Fetches a URL with `reqwest` and parses the HTML with `scraper`. External
//! stylesheets referenced by `<link rel="stylesheet">` are fetched
//! concurrently and returned alongside any inline `<style>` blocks.
//!
//! ## Input
//! - `&url::Url` — fully qualified target URL
//!
//! ## Output
//! - `ClonedSite { html, base_url, inline_styles, external_styles }`
//!
//! ## Limitations
//! - No JavaScript execution. SPAs return their pre-hydration shell only.
//!   See `Phase 3` of the plan for `chromiumoxide` integration.
//!
//! ## Related
//! - `forge-canvas::dom::DomTree::from_html` — primary downstream consumer
//! - `forge-canvas::tsx_writer::emit_project` — uses the CSS payload

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
