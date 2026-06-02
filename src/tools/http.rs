//! HTTP Operations

use anyhow::Result;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::time::{Duration, Instant};
use tokio::fs;
use tracing::info;

/// Make HTTP request
pub async fn request(args: Value) -> Result<Value> {
    let url = args.get("url").and_then(|v| v.as_str()).unwrap_or("");
    let method = args.get("method").and_then(|v| v.as_str()).unwrap_or("GET");
    let headers: HashMap<String, String> = args
        .get("headers")
        .and_then(|v| serde_json::from_value(v.clone()).ok())
        .unwrap_or_default();
    let body = args.get("body").and_then(|v| v.as_str());
    let timeout_secs = args.get("timeout").and_then(|v| v.as_u64()).unwrap_or(30);

    if url.is_empty() {
        anyhow::bail!("url is required");
    }

    info!("HTTP {} {}", method, url);

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(timeout_secs))
        .build()?;

    let start = Instant::now();

    let mut request = match method.to_uppercase().as_str() {
        "GET" => client.get(url),
        "POST" => client.post(url),
        "PUT" => client.put(url),
        "DELETE" => client.delete(url),
        "PATCH" => client.patch(url),
        "HEAD" => client.head(url),
        _ => anyhow::bail!("Unsupported method: {}", method),
    };

    for (key, value) in headers {
        request = request.header(&key, &value);
    }

    if let Some(b) = body {
        request = request.body(b.to_string());
    }

    let response = request.send().await?;
    let elapsed = start.elapsed().as_millis() as u64;

    let status = response.status().as_u16();
    let response_headers: HashMap<String, String> = response
        .headers()
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_str().unwrap_or("").to_string()))
        .collect();

    let body_text = response.text().await?;

    Ok(json!({
        "success": status >= 200 && status < 300,
        "status_code": status,
        "headers": response_headers,
        "body": body_text,
        "body_length": body_text.len(),
        "response_time_ms": elapsed
    }))
}

/// Download file
pub async fn download(args: Value) -> Result<Value> {
    let url = args.get("url").and_then(|v| v.as_str()).unwrap_or("");
    let destination = args
        .get("destination")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let resume = args.get("resume").and_then(|v| v.as_bool()).unwrap_or(true);

    if url.is_empty() || destination.is_empty() {
        anyhow::bail!("url and destination are required");
    }

    info!("Downloading {} -> {}", url, destination);

    // Create parent directories
    if let Some(parent) = std::path::Path::new(destination).parent() {
        let _ = fs::create_dir_all(parent).await;
    }

    let client = reqwest::Client::new();
    let start = Instant::now();

    // Check for existing file if resume enabled
    let existing_size = if resume {
        fs::metadata(destination).await.map(|m| m.len()).ok()
    } else {
        None
    };

    let mut request = client.get(url);

    if let Some(size) = existing_size {
        if size > 0 {
            request = request.header("Range", format!("bytes={}-", size));
        }
    }

    let response = request.send().await?;
    let status = response.status();

    if !status.is_success() && status.as_u16() != 206 {
        return Ok(json!({
            "success": false,
            "error": format!("HTTP {}", status)
        }));
    }

    let _total_size = response.content_length().unwrap_or(0);
    let bytes = response.bytes().await?;

    // Write file (append if resuming)
    if existing_size.is_some() && status.as_u16() == 206 {
        let mut file = tokio::fs::OpenOptions::new()
            .append(true)
            .open(destination)
            .await?;
        tokio::io::AsyncWriteExt::write_all(&mut file, &bytes).await?;
    } else {
        fs::write(destination, &bytes).await?;
    }

    let elapsed = start.elapsed().as_millis() as u64;
    let final_size = fs::metadata(destination)
        .await
        .map(|m| m.len())
        .unwrap_or(0);

    Ok(json!({
        "success": true,
        "destination": destination,
        "size_bytes": final_size,
        "download_time_ms": elapsed,
        "resumed": existing_size.is_some() && status.as_u16() == 206
    }))
}

pub async fn scrape(args: Value) -> Result<Value> {
    let url = args.get("url").and_then(|v| v.as_str()).unwrap_or("");
    let _selector = args.get("selector").and_then(|v| v.as_str());

    if url.is_empty() {
        anyhow::bail!("url is required");
    }

    let client = reqwest::Client::new();
    let response = client.get(url).send().await?;
    let status = response.status().as_u16();
    let html = response.text().await?;

    // Strip HTML tags to get text content
    let mut text = String::new();
    let mut in_tag = false;
    let mut in_script = false;
    let mut in_style = false;
    let lower_html = html.to_lowercase();
    let chars: Vec<char> = html.chars().collect();
    let lower_chars: Vec<char> = lower_html.chars().collect();

    let mut i = 0;
    while i < chars.len() {
        if !in_tag && chars[i] == '<' {
            in_tag = true;
            // Check for script/style tags
            let remaining: String = lower_chars[i..].iter().take(10).collect();
            if remaining.starts_with("<script") {
                in_script = true;
            }
            if remaining.starts_with("<style") {
                in_style = true;
            }
            if remaining.starts_with("</script") {
                in_script = false;
            }
            if remaining.starts_with("</style") {
                in_style = false;
            }
        } else if in_tag && chars[i] == '>' {
            in_tag = false;
        } else if !in_tag && !in_script && !in_style {
            text.push(chars[i]);
        }
        i += 1;
    }

    // Clean up whitespace
    let cleaned: String = text
        .lines()
        .map(|l| l.trim())
        .filter(|l| !l.is_empty())
        .collect::<Vec<&str>>()
        .join("\n");

    Ok(json!({
        "url": url,
        "status": status,
        "text": if cleaned.len() > 50000 { &cleaned[..50000] } else { &cleaned },
        "truncated": cleaned.len() > 50000,
        "original_length": html.len(),
        "text_length": cleaned.len()
    }))
}
