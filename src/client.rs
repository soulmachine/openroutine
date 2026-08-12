//! Talking to the local daemon.
//!
//! Small deliberately: the destination is always loopback HTTP/1.1 on this
//! machine, so a general HTTP client would be weight for nothing — and
//! shelling out to `curl` would make a program that promises no runtime
//! dependencies quietly depend on one.

use anyhow::{Context, Result};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// What the daemon answered.
pub struct Answer {
    pub status: u16,
    pub body: String,
}

impl Answer {
    /// The body as JSON, or an empty object if it isn't.
    pub fn json(&self) -> serde_json::Value {
        serde_json::from_str(&self.body).unwrap_or_else(|_| serde_json::json!({}))
    }

    pub fn ok(&self) -> bool {
        (200..300).contains(&self.status)
    }
}

/// Sends one request and reads the whole reply.
pub async fn send(
    address: &str,
    token: &str,
    method: &str,
    path: &str,
    body: Option<&str>,
) -> Result<Answer> {
    let mut stream = tokio::net::TcpStream::connect(address)
        .await
        .with_context(|| format!("connecting to the daemon at {address}"))?;

    let payload = body.unwrap_or_default();
    let mut request = format!(
        "{method} {path} HTTP/1.1\r\n\
         Host: {address}\r\n\
         Authorization: Bearer {token}\r\n\
         Connection: close\r\n\
         Content-Length: {}\r\n",
        payload.len()
    );
    if body.is_some() {
        request.push_str("Content-Type: application/json\r\n");
    }
    request.push_str("\r\n");
    request.push_str(payload);

    stream
        .write_all(request.as_bytes())
        .await
        .context("sending the request")?;

    let mut raw = Vec::new();
    stream
        .read_to_end(&mut raw)
        .await
        .context("reading the reply")?;
    let raw = String::from_utf8_lossy(&raw);

    let status = raw
        .split_whitespace()
        .nth(1)
        .and_then(|code| code.parse().ok())
        .context("the daemon's reply had no status line")?;
    let body = raw
        .split_once("\r\n\r\n")
        .map(|(_, body)| body.to_string())
        .unwrap_or_default();

    Ok(Answer { status, body })
}
