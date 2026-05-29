use crate::core::{CoAIError, Result};
use futures::StreamExt;
use once_cell::sync::OnceCell;
use reqwest::{Client, Method};
use std::collections::HashMap;
use std::io::Write;
use std::path::PathBuf;
use std::time::{Duration, Instant};

const DEFAULT_HTTP_TIMEOUT_SECS: u64 = 8;
const DEFAULT_HTTP_CONNECT_TIMEOUT_SECS: u64 = 3;
const DEFAULT_HTTP_MAX_BYTES: usize = 768 * 1024;
const DEFAULT_SEARCH_MAX_BYTES: usize = 512 * 1024;

pub struct NetTools {
    #[allow(dead_code)]
    workspace: PathBuf,
    log_path: PathBuf,
}

impl NetTools {
    pub fn new(workspace: impl Into<PathBuf>) -> Self {
        let ws = workspace.into();
        let log_path = ws.join(".coai/state").join("http.log");
        Self {
            workspace: ws,
            log_path,
        }
    }

    fn log_request(
        &self,
        method: &str,
        url: &str,
        status: u16,
        size: usize,
        elapsed_ms: u64,
        err: Option<&str>,
    ) {
        let _ = (|| -> std::io::Result<()> {
            if let Some(parent) = self.log_path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            let now = chrono::Local::now().format("%m-%d %H:%M:%S%.3f");
            let size_kb = size as f64 / 1024.0;
            let line = match err {
                Some(e) => format!(
                    "[{}] {} {} → ERR {} ({:.1}s)\n",
                    now,
                    method,
                    url,
                    e,
                    elapsed_ms as f64 / 1000.0
                ),
                None => format!(
                    "[{}] {} {} → {} {:.1}KB {:.1}s\n",
                    now,
                    method,
                    url,
                    status,
                    size_kb,
                    elapsed_ms as f64 / 1000.0
                ),
            };
            let mut f = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&self.log_path)?;
            f.write_all(line.as_bytes())?;
            Ok(())
        })();
    }

    /// Open a URL in the default browser.
    pub fn open_browser(&self, url: &str) -> Result<String> {
        let cmd = if cfg!(target_os = "macos") {
            ("open", url.to_string())
        } else if cfg!(target_os = "linux") {
            ("xdg-open", url.to_string())
        } else if cfg!(target_os = "windows") {
            ("cmd", format!("/c start {}", url))
        } else {
            return Err(CoAIError::Other("Unsupported operating system".into()));
        };
        let output = std::process::Command::new(cmd.0)
            .arg(&cmd.1)
            .output()
            .map_err(|e| CoAIError::Other(format!("Failed to open browser: {}", e)))?;
        if output.status.success() {
            Ok(format!("Opened in browser: {}", url))
        } else {
            let err = String::from_utf8_lossy(&output.stderr);
            Err(CoAIError::Other(format!("Failed to open browser: {}", err)))
        }
    }

    pub async fn http_get(&self, url: &str) -> Result<String> {
        let t0 = Instant::now();
        let client = build_http_client()?;
        let response = match client.get(url).send().await {
            Ok(r) => r,
            Err(e) => {
                let elapsed = t0.elapsed().as_millis() as u64;
                self.log_request("GET", url, 0, 0, elapsed, Some(&e.to_string()));
                return Err(CoAIError::Other(format!("HTTP GET request failed: {}", e)));
            }
        };

        let status = response.status().as_u16();
        if !response.status().is_success() {
            let elapsed = t0.elapsed().as_millis() as u64;
            self.log_request("GET", url, status, 0, elapsed, None);
            return Err(CoAIError::Other(format!(
                "HTTP GET request failed, status: {}",
                response.status()
            )));
        }

        let (body, truncated) = read_text_limited(response, http_max_bytes()).await?;
        let body = extract_readable_text(&body);

        let body = if truncated {
            format!(
                "{}\n\n[Content truncated at {}KB. Use a more specific URL or fetch in parts.]",
                body,
                http_max_bytes() / 1024
            )
        } else {
            body
        };

        let elapsed = t0.elapsed().as_millis() as u64;
        self.log_request("GET", url, status, body.len(), elapsed, None);
        Ok(body)
    }

    pub async fn http_post(&self, url: &str, body: &str) -> Result<String> {
        let t0 = Instant::now();
        let client = build_http_client()?;
        let response = match client
            .post(url)
            .body(body.to_string())
            .header("Content-Type", "application/json")
            .send()
            .await
        {
            Ok(r) => r,
            Err(e) => {
                let elapsed = t0.elapsed().as_millis() as u64;
                self.log_request("POST", url, 0, 0, elapsed, Some(&e.to_string()));
                return Err(CoAIError::Other(format!("HTTP POST request failed: {}", e)));
            }
        };

        let status = response.status().as_u16();
        if !response.status().is_success() {
            let elapsed = t0.elapsed().as_millis() as u64;
            self.log_request("POST", url, status, 0, elapsed, None);
            return Err(CoAIError::Other(format!(
                "HTTP POST request failed, status: {}",
                response.status()
            )));
        }

        let (response_body, truncated) = read_text_limited(response, http_max_bytes()).await?;
        let response_body = if truncated {
            format!(
                "{}\n\n[Content truncated at {}KB.]",
                response_body,
                http_max_bytes() / 1024
            )
        } else {
            response_body
        };

        let elapsed = t0.elapsed().as_millis() as u64;
        self.log_request("POST", url, status, response_body.len(), elapsed, None);
        Ok(response_body)
    }

    pub async fn http_request(
        &self,
        method: &str,
        url: &str,
        headers: Option<HashMap<String, String>>,
        body: Option<String>,
    ) -> Result<String> {
        let t0 = Instant::now();
        let client = build_http_client()?;
        let method_enum = Method::from_bytes(method.to_uppercase().as_bytes())
            .map_err(|e| CoAIError::Other(format!("Invalid HTTP method: {}", e)))?;

        let mut request = client.request(method_enum, url);

        if let Some(headers) = headers {
            for (key, value) in headers {
                request = request.header(&key, value);
            }
        }

        if let Some(body_content) = body {
            request = request.body(body_content);
        }

        let response = match request.send().await {
            Ok(r) => r,
            Err(e) => {
                let elapsed = t0.elapsed().as_millis() as u64;
                self.log_request(method, url, 0, 0, elapsed, Some(&e.to_string()));
                return Err(CoAIError::Other(format!("HTTP request failed: {}", e)));
            }
        };

        let status = response.status();
        let resp_headers: HashMap<String, String> = response
            .headers()
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_str().unwrap_or("").to_string()))
            .collect();

        let (response_body, truncated) = read_text_limited(response, http_max_bytes()).await?;
        let response_body = if truncated {
            format!(
                "{}\n\n[Content truncated at {}KB.]",
                response_body,
                http_max_bytes() / 1024
            )
        } else {
            response_body
        };

        let elapsed = t0.elapsed().as_millis() as u64;
        self.log_request(
            method,
            url,
            status.as_u16(),
            response_body.len(),
            elapsed,
            None,
        );

        let result = serde_json::json!({
            "status_code": status.as_u16(),
            "status_text": status.to_string(),
            "headers": resp_headers,
            "body": response_body,
        });

        Ok(serde_json::to_string(&result)?)
    }

    /// Search the web using Sogou and return formatted results.
    pub async fn web_search(&self, query: &str) -> Result<String> {
        let t0 = Instant::now();
        let client = build_http_client()?;

        let url = format!("https://www.sogou.com/web?query={}", urlencoding(query));

        let response = match client.get(&url).send().await {
            Ok(r) => r,
            Err(e) => {
                let elapsed = t0.elapsed().as_millis() as u64;
                self.log_request("SEARCH", &url, 0, 0, elapsed, Some(&e.to_string()));
                return Err(CoAIError::Other(format!("Search request failed: {}", e)));
            }
        };

        let status = response.status().as_u16();
        let html = match read_text_limited(response, search_max_bytes()).await {
            Ok((h, _)) => h,
            Err(e) => {
                let elapsed = t0.elapsed().as_millis() as u64;
                self.log_request("SEARCH", &url, status, 0, elapsed, Some(&e.to_string()));
                return Err(CoAIError::Other(format!("Failed to read search response: {}", e)));
            }
        };

        let elapsed = t0.elapsed().as_millis() as u64;
        self.log_request("SEARCH", &url, status, html.len(), elapsed, None);

        let results = parse_search_html(&html);

        let mut output = String::new();
        output.push_str(&format!("Search: {}\n\n", query));

        if results.is_empty() {
            output.push_str("No search results found. Try different keywords or visit a known site directly.\n");
        } else {
            for (i, r) in results.iter().enumerate() {
                output.push_str(&format!(
                    "{}. {}\n   URL: {}\n   {}\n\n",
                    i + 1,
                    r.title,
                    r.url,
                    r.snippet
                ));
            }
        }

        Ok(output)
    }
}

struct SearchResult {
    title: String,
    url: String,
    snippet: String,
}

/// Simple search result extractor. Extracts links with descriptive text from HTML.
/// Focuses on high-quality results and limits output size.
fn parse_search_html(html: &str) -> Vec<SearchResult> {
    let mut results = Vec::new();
    let mut pos = 0;

    // Extract links that look like search results
    while pos < html.len() && results.len() < 8 {
        if let Some(a_start) = html[pos..].find("<a ") {
            let abs_start = pos + a_start;

            if let Some(href_start) = html[abs_start..].find("href=\"") {
                let href_val = &html[abs_start + href_start + 6..];
                let url = if let Some(end) = href_val.find('"') {
                    href_val[..end].to_string()
                } else {
                    String::new()
                };

                // Only keep external http(s) URLs
                let is_good_url = url.starts_with("http")
                    && !url.contains("sogou.com")
                    && !url.contains("baidu.com")
                    && !url.contains("doubleclick")
                    && url.len() > 20;

                if is_good_url {
                    // Get link text
                    let after_href = &href_val;
                    if let Some(gt) = after_href.find('>') {
                        let content_start = abs_start + href_start + 6 + gt + 1;
                        if let Some(a_end) = html[content_start..].find("</a>") {
                            let title = clean_html(&html[content_start..content_start + a_end])
                                .trim()
                                .to_string();

                            if title.len() > 4
                                && title.len() < 300
                                && is_chinese_or_english_title(&title)
                                && !is_noise_title(&title)
                            {
                                if !results.iter().any(|r: &SearchResult| r.title == title) {
                                    results.push(SearchResult {
                                        title,
                                        url,
                                        snippet: String::new(),
                                    });
                                }
                            }
                        }
                    }
                }
            }
            pos = abs_start + 10;
        } else {
            break;
        }
    }

    results
}

fn is_chinese_or_english_title(title: &str) -> bool {
    let has_chinese = title.chars().any(|c| c as u32 > 0x4E00);
    let has_english = title.chars().any(|c| c.is_ascii_alphabetic());
    has_chinese || has_english
}

fn is_noise_title(title: &str) -> bool {
    // Filter out common noise patterns in search results
    let noise_patterns = [
        "ICP",
        "备案",
        "京ICP",
        "许可证",
        "B2-",
        "合作伙伴",
        "友情链接",
        "关于我们",
        "联系我们",
        "侵权",
        "举报",
        "广告",
        "合作",
        "登录",
        "注册",
        "首页",
        "上一页",
        "下一页",
        "末页",
        "触屏版",
        "电脑版",
        "客户端",
        "APP",
    ];
    for pattern in &noise_patterns {
        if title.contains(pattern) {
            return true;
        }
    }
    // Filter URLs that look like they're not content pages
    if title.starts_with("http") || title.starts_with("www.") {
        return true;
    }
    false
}

/// Parse Baidu HTML search results page.
#[allow(dead_code)]
fn parse_baidu_html(html: &str) -> Vec<SearchResult> {
    let mut results = Vec::new();
    let mut search_start = 0;

    while let Some(pos) = html[search_start..].find("class=\"c-container") {
        let chunk = &html[search_start + pos..];
        // Limit chunk size for each result
        let end = chunk.find("class=\"c-container").unwrap_or(10000);
        let result_chunk = &chunk[..end.min(chunk.len())];

        // Extract title from <h3> tag
        let title = if let Some(h3_start) = result_chunk.find("<h3") {
            if let Some(h3_content_start) = result_chunk[h3_start..].find('>') {
                let after_h3 = &result_chunk[h3_start + h3_content_start + 1..];
                if let Some(h3_end) = after_h3.find("</h3>") {
                    clean_html(&after_h3[..h3_end])
                } else {
                    String::new()
                }
            } else {
                String::new()
            }
        } else {
            String::new()
        };

        // Extract URL from various patterns
        let url = if let Some(showurl_start) = result_chunk.find("class=\"c-showurl\"") {
            extract_inner_text(&result_chunk[showurl_start..])
        } else {
            // Try to find the first href in the chunk
            if let Some(href_start) = result_chunk.find("href=\"") {
                let after_href = &result_chunk[href_start + 6..];
                if let Some(quote_end) = after_href.find('"') {
                    let link = &after_href[..quote_end];
                    if link.starts_with("http") && !link.contains("baidu.com") {
                        link.to_string()
                    } else {
                        String::new()
                    }
                } else {
                    String::new()
                }
            } else {
                String::new()
            }
        };

        // Extract snippet from <span class="content-right_8Zs40">
        let snippet = if let Some(content_start) = result_chunk.find("content-right") {
            extract_inner_text(&result_chunk[content_start..])
        } else if let Some(abs_start) = result_chunk.find("c-abstract") {
            extract_inner_text(&result_chunk[abs_start..])
        } else if result_chunk.find("<span").is_some() {
            // Find the last span with text content
            let spans: Vec<String> = result_chunk
                .match_indices("<span")
                .map(|(i, _)| extract_inner_text(&result_chunk[i..]))
                .collect();
            spans.last().cloned().unwrap_or_default()
        } else {
            String::new()
        };

        let cleaned_title = clean_html(&title);
        let cleaned_snippet = clean_html(&snippet);

        if !cleaned_title.is_empty() {
            results.push(SearchResult {
                title: cleaned_title,
                url,
                snippet: cleaned_snippet,
            });
        }

        search_start += pos + 100;
        if results.len() >= 10 {
            break;
        }
    }

    results
}

/// Parse Bing HTML search results page.
#[allow(dead_code)]
fn parse_duckduckgo_html(html: &str) -> Vec<SearchResult> {
    let mut results = Vec::new();
    let mut titles: Vec<(String, String)> = Vec::new();
    let mut snippets: Vec<String> = Vec::new();
    let mut search_start = 0;

    while let Some(pos) = html[search_start..].find("class=\"b_algo\"") {
        let chunk = &html[search_start + pos..];
        let url = if let Some(link_start) = chunk.find("<h2>") {
            let after_h2 = &chunk[link_start..];
            if let Some(href_start) = after_h2.find("href=\"") {
                let after_href = &after_h2[href_start + 6..];
                if let Some(quote_end) = after_href.find('"') {
                    after_href[..quote_end].to_string()
                } else {
                    String::new()
                }
            } else {
                String::new()
            }
        } else {
            String::new()
        };
        let title = extract_inner_text(chunk);
        let snippet = if let Some(snip_start) = chunk.find("class=\"b_lineclamp") {
            extract_inner_text(&chunk[snip_start..])
        } else if let Some(p_start) = chunk.rfind("<p") {
            let after_p = &chunk[p_start..];
            extract_inner_text(after_p)
        } else {
            String::new()
        };
        if !title.is_empty() && !url.is_empty() {
            titles.push((url, clean_html(&title)));
            snippets.push(clean_html(&snippet));
        }
        search_start += pos + 50;
        if titles.len() >= 10 {
            break;
        }
    }

    let count = titles.len().min(snippets.len()).min(10);
    for i in 0..count {
        results.push(SearchResult {
            title: titles[i].1.clone(),
            url: titles[i].0.clone(),
            snippet: snippets.get(i).cloned().unwrap_or_default(),
        });
    }
    results
}

/// Extract an HTML attribute value like href="..." from a chunk starting at the tag
#[allow(dead_code)]
fn extract_attr(chunk: &str, attr: &str) -> String {
    let prefix = format!("{}=\"", attr);
    if let Some(start) = chunk.find(&prefix) {
        let after = &chunk[start + prefix.len()..];
        if let Some(end) = after.find('"') {
            return after[..end].to_string();
        }
    }
    String::new()
}

/// Extract text content between > and < from a chunk starting at a tag
#[allow(dead_code)]
fn extract_inner_text(chunk: &str) -> String {
    if let Some(start) = chunk.find('>') {
        let after = &chunk[start + 1..];
        if let Some(end) = after.find('<') {
            return after[..end].to_string();
        }
        // If no closing < found, return rest (up to reasonable length)
        return after.chars().take(500).collect();
    }
    String::new()
}

#[allow(dead_code)]
fn clean_url(url: &str) -> String {
    // DuckDuckGo wraps URLs with a redirect, extract the real URL
    if let Some(start) = url.find("uddg=") {
        let encoded = &url[start + 5..];
        let decoded = url_decode(encoded);
        // Remove trailing parameters after &
        if let Some(amp_pos) = decoded.find("&rut=") {
            decoded[..amp_pos].to_string()
        } else {
            decoded
        }
    } else {
        url.to_string()
    }
}

#[allow(dead_code)]
fn url_decode(s: &str) -> String {
    let mut result = String::new();
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'%' if i + 2 < bytes.len() => {
                if let Ok(hex) = u8::from_str_radix(
                    &std::str::from_utf8(&bytes[i + 1..i + 3]).unwrap_or("00"),
                    16,
                ) {
                    result.push(hex as char);
                    i += 3;
                    continue;
                }
            }
            b'+' => {
                result.push(' ');
                i += 1;
                continue;
            }
            c => {
                result.push(c as char);
            }
        }
        i += 1;
    }
    result
}

fn clean_html(text: &str) -> String {
    // Remove HTML tags
    let mut result = String::new();
    let mut in_tag = false;
    for c in text.chars() {
        match c {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => result.push(c),
            _ => {}
        }
    }
    // Collapse whitespace
    let result = result.split_whitespace().collect::<Vec<_>>().join(" ");
    result
}

/// Detect HTML and extract readable text content instead of raw tags.
/// If the body doesn't look like HTML, return it as-is.
fn extract_readable_text(body: &str) -> String {
    let trimmed = body.trim();

    // Quick check: does this look like HTML?
    if !trimmed.starts_with("<!DOCTYPE")
        && !trimmed.starts_with("<html")
        && !trimmed.starts_with("<HTML")
    {
        // Not HTML — return as-is
        return body.to_string();
    }

    let mut result = String::new();

    // Extract <title>
    if let Some(title) = extract_tag_content(trimmed, "title") {
        if !title.is_empty() {
            result.push_str(&format!("Title: {}\n\n", title.trim()));
        }
    }

    // Extract <meta name="description">
    if let Some(desc) = extract_meta_description(trimmed) {
        if !desc.is_empty() {
            result.push_str(&format!("Description: {}\n\n", desc.trim()));
        }
    }

    // Extract body content with structure awareness
    let body_content = extract_body(trimmed);

    // Parse and format key elements
    let mut pos = 0;
    let mut lines: Vec<String> = Vec::new();

    while pos < body_content.len() {
        // Skip script and style blocks entirely
        if let Some(end) = skip_block(&body_content, pos, "script") {
            pos = end;
            continue;
        }
        if let Some(end) = skip_block(&body_content, pos, "style") {
            pos = end;
            continue;
        }

        // Headings
        for level in 1..=6 {
            let tag = format!("h{}", level);
            if let Some((text, end)) = extract_tag_at(&body_content, pos, &tag) {
                if !text.trim().is_empty() {
                    let prefix = "#".repeat(level) + " ";
                    lines.push(format!("{}{}", prefix, clean_html(&text).trim()));
                }
                pos = end;
                continue;
            }
        }

        // Paragraphs
        if let Some((text, end)) = extract_tag_at(&body_content, pos, "p") {
            let cleaned = clean_html(&text).trim().to_string();
            if !cleaned.is_empty() && cleaned.len() > 2 {
                lines.push(cleaned);
            }
            pos = end;
            continue;
        }

        // List items
        if let Some((text, end)) = extract_tag_at(&body_content, pos, "li") {
            let cleaned = clean_html(&text).trim().to_string();
            if !cleaned.is_empty() {
                lines.push(format!("• {}", cleaned));
            }
            pos = end;
            continue;
        }

        // Advance past any tag
        if body_content[pos..].starts_with('<') {
            if let Some(gt) = body_content[pos..].find('>') {
                pos += gt + 1;
            } else {
                pos += 1;
            }
        } else {
            // Plain text — collect until next tag
            let end = body_content[pos..]
                .find('<')
                .unwrap_or(body_content.len() - pos);
            let text = body_content[pos..pos + end].trim();
            if !text.is_empty() && text.len() > 2 {
                lines.push(clean_html(text));
            }
            pos += end;
        }
    }

    // Join lines and collapse excessive blank lines
    result.push_str(&lines.join("\n\n"));

    // Collapse multiple blank lines
    while result.contains("\n\n\n") {
        result = result.replace("\n\n\n", "\n\n");
    }

    // Limit output size
    let max_chars = 4000;
    if result.len() > max_chars {
        let truncated = result[..result.floor_char_boundary(max_chars)].to_string();
        format!(
            "{}\n\n[Content condensed; original is longer. Use a more specific URL for full content.]",
            truncated.trim()
        )
    } else {
        result
    }
}

fn extract_tag_content(html: &str, tag: &str) -> Option<String> {
    let open = format!("<{}", tag);
    let start = html.find(&open)?;
    let gt = html[start..].find('>')?;
    let content_start = start + gt + 1;
    let close = format!("</{}>", tag);
    let end = html[content_start..].find(&close)?;
    Some(html[content_start..content_start + end].to_string())
}

fn extract_meta_description(html: &str) -> Option<String> {
    let needle = "name=\"description\"";
    let pos = html.find(needle)?;
    // Look for content="..." after this point
    let rest = &html[pos..];
    let content_attr = rest.find("content=\"")?;
    let val_start = pos + content_attr + 9;
    let val_end = html[val_start..].find('"')?;
    Some(html[val_start..val_start + val_end].to_string())
}

fn extract_body(html: &str) -> String {
    if let Some(body_start) = html.find("<body") {
        let gt = html[body_start..].find('>').unwrap_or(0);
        let content_start = body_start + gt + 1;
        if let Some(body_end) = html[content_start..].find("</body>") {
            return html[content_start..content_start + body_end].to_string();
        }
        return html[content_start..].to_string();
    }
    html.to_string()
}

fn skip_block(html: &str, pos: usize, tag: &str) -> Option<usize> {
    let open = format!("<{}", tag);
    if !html[pos..].starts_with(&open) && !html[pos..].starts_with(&format!("<{} ", tag)) {
        return None;
    }
    let close = format!("</{}>", tag);
    let end = html[pos..].find(&close)?;
    Some(pos + end + close.len())
}

fn extract_tag_at(html: &str, pos: usize, tag: &str) -> Option<(String, usize)> {
    let open1 = format!("<{}>", tag);
    let open2 = format!("<{} ", tag);
    let remaining = &html[pos..];

    if !remaining.starts_with(&open1) && !remaining.starts_with(&open2) {
        return None;
    }

    let gt = remaining.find('>')?;
    let content_start = pos + gt + 1;
    let close = format!("</{}>", tag);
    let end = html[content_start..].find(&close)?;
    Some((
        html[content_start..content_start + end].to_string(),
        content_start + end + close.len(),
    ))
}

fn build_http_client() -> Result<Client> {
    static CLIENT: OnceCell<Client> = OnceCell::new();
    CLIENT
        .get_or_try_init(|| {
            Client::builder()
        .user_agent("Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36")
        .connect_timeout(Duration::from_secs(http_connect_timeout_secs()))
        .timeout(Duration::from_secs(http_timeout_secs()))
        .redirect(reqwest::redirect::Policy::limited(3))
        .build()
        .map_err(|e| CoAIError::Other(format!("Failed to create HTTP client: {}", e)))
        })
        .cloned()
}

async fn read_text_limited(
    response: reqwest::Response,
    max_bytes: usize,
) -> Result<(String, bool)> {
    let mut bytes = Vec::new();
    let mut stream = response.bytes_stream();
    let mut truncated = false;

    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| CoAIError::Other(format!("Failed to read response body: {}", e)))?;
        let remaining = max_bytes.saturating_sub(bytes.len());
        if chunk.len() > remaining {
            bytes.extend_from_slice(&chunk[..remaining]);
            truncated = true;
            break;
        }
        bytes.extend_from_slice(&chunk);
        if bytes.len() >= max_bytes {
            truncated = true;
            break;
        }
    }

    Ok((String::from_utf8_lossy(&bytes).to_string(), truncated))
}

fn http_timeout_secs() -> u64 {
    std::env::var("COAI_HTTP_TIMEOUT_SECS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(DEFAULT_HTTP_TIMEOUT_SECS)
}

fn http_connect_timeout_secs() -> u64 {
    std::env::var("COAI_HTTP_CONNECT_TIMEOUT_SECS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(DEFAULT_HTTP_CONNECT_TIMEOUT_SECS)
}

fn http_max_bytes() -> usize {
    std::env::var("COAI_HTTP_MAX_BYTES")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(DEFAULT_HTTP_MAX_BYTES)
}

fn search_max_bytes() -> usize {
    std::env::var("COAI_SEARCH_MAX_BYTES")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(DEFAULT_SEARCH_MAX_BYTES)
}

fn urlencoding(s: &str) -> String {
    let mut result = String::new();
    for byte in s.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                result.push(byte as char);
            }
            b' ' => result.push('+'),
            _ => {
                result.push('%');
                result.push_str(&format!("{:02X}", byte));
            }
        }
    }
    result
}
