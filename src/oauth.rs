use std::time::{SystemTime, UNIX_EPOCH};

use serde::Deserialize;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

use crate::error::{BotError, BotResult};

const AUTHORIZE_URL: &str = "https://id.twitch.tv/oauth2/authorize";
const TOKEN_URL: &str = "https://id.twitch.tv/oauth2/token";
const DEFAULT_REDIRECT: &str = "http://localhost:3000";
const DEFAULT_SCOPES: &[&str] = &["chat:read", "chat:edit", "moderator:read:followers"];

#[derive(Debug, Deserialize)]
struct TokenResponse {
    access_token: String,
    refresh_token: String,
    #[serde(default)]
    expires_in: u64,
    #[serde(default)]
    scope: Vec<String>,
}

pub async fn run() -> BotResult<()> {
    crate::config::load_dotenv(".env");

    let client_id = require("TWITCH_CLIENT_ID")?;
    let client_secret = require("TWITCH_CLIENT_SECRET")?;
    let redirect_uri =
        std::env::var("TWITCH_REDIRECT_URI").unwrap_or_else(|_| DEFAULT_REDIRECT.to_string());
    let scopes = DEFAULT_SCOPES.join(" ");

    let (bind_host, port, _path) = parse_redirect(&redirect_uri)?;
    let listener = TcpListener::bind((bind_host.as_str(), port)).await?;

    let state = random_state();
    let authorize_url = format!(
        "{AUTHORIZE_URL}?response_type=code&client_id={}&redirect_uri={}&scope={}&state={}",
        encode(&client_id),
        encode(&redirect_uri),
        encode(&scopes),
        encode(&state),
    );

    println!("\nOpen this URL in your browser and log in with the BOT account:\n");
    println!("  {authorize_url}\n");
    open_browser(&authorize_url);
    println!("Waiting for the redirect on {redirect_uri} ...\n");

    let code = wait_for_code(&listener, &state).await?;

    let token = exchange_code(&client_id, &client_secret, &redirect_uri, &code).await?;

    patch_env_file(&[
        ("TWITCH_OAUTH_TOKEN", &token.access_token),
        ("TWITCH_REFRESH_TOKEN", &token.refresh_token),
    ])?;

    println!("Authorization successful.");
    println!("  scopes:     {}", token.scope.join(", "));
    println!("  expires in: {}s", token.expires_in);
    println!("\nTWITCH_OAUTH_TOKEN and TWITCH_REFRESH_TOKEN written to .env.");
    println!("You can now run: cargo run\n");

    Ok(())
}

async fn wait_for_code(listener: &TcpListener, expected_state: &str) -> BotResult<String> {
    loop {
        let (mut socket, _) = listener.accept().await?;

        let mut buffer = [0u8; 4096];
        let read = socket.read(&mut buffer).await?;
        let request = String::from_utf8_lossy(&buffer[..read]);

        let Some(target) = request_target(&request) else {
            respond(&mut socket, "400 Bad Request", "bad request").await;
            continue;
        };

        let Some(query) = target.split_once('?').map(|(_, q)| q) else {
            respond(&mut socket, "404 Not Found", "waiting for the twitch redirect").await;
            continue;
        };

        let params = parse_query(query);

        if let Some(error) = params.iter().find(|(k, _)| k == "error") {
            let description = params
                .iter()
                .find(|(k, _)| k == "error_description")
                .map(|(_, v)| v.as_str())
                .unwrap_or("");
            respond(&mut socket, "200 OK", "authorization denied, you can close this tab").await;
            return Err(BotError::Auth(format!("{}: {description}", error.1)));
        }

        let Some((_, code)) = params.iter().find(|(k, _)| k == "code") else {
            respond(&mut socket, "404 Not Found", "waiting for the twitch redirect").await;
            continue;
        };

        let returned_state = params
            .iter()
            .find(|(k, _)| k == "state")
            .map(|(_, v)| v.as_str())
            .unwrap_or("");
        if returned_state != expected_state {
            respond(&mut socket, "400 Bad Request", "state mismatch").await;
            return Err(BotError::Auth("oauth state mismatch (possible csrf)".into()));
        }

        respond(
            &mut socket,
            "200 OK",
            "authorization complete, you can close this tab and return to the terminal",
        )
        .await;
        return Ok(code.clone());
    }
}

async fn exchange_code(
    client_id: &str,
    client_secret: &str,
    redirect_uri: &str,
    code: &str,
) -> BotResult<TokenResponse> {
    let http = reqwest::Client::new();
    let response = http
        .post(TOKEN_URL)
        .form(&[
            ("grant_type", "authorization_code"),
            ("client_id", client_id),
            ("client_secret", client_secret),
            ("code", code),
            ("redirect_uri", redirect_uri),
        ])
        .send()
        .await?;

    if !response.status().is_success() {
        let status = response.status();
        let detail = response.text().await.unwrap_or_default();
        return Err(BotError::Auth(format!(
            "token exchange failed ({status}): {detail}"
        )));
    }

    Ok(response.json().await?)
}

fn patch_env_file(updates: &[(&str, &str)]) -> BotResult<()> {
    let path = ".env";
    let existing = std::fs::read_to_string(path).unwrap_or_default();
    let mut lines: Vec<String> = existing.lines().map(|l| l.to_string()).collect();

    for (key, value) in updates {
        let prefix = format!("{key}=");
        match lines.iter_mut().find(|l| l.trim_start().starts_with(&prefix)) {
            Some(line) => *line = format!("{key}={value}"),
            None => lines.push(format!("{key}={value}")),
        }
    }

    let mut out = lines.join("\n");
    out.push('\n');
    std::fs::write(path, out)?;
    Ok(())
}

fn request_target(request: &str) -> Option<&str> {
    let mut parts = request.split_whitespace();
    let _method = parts.next()?;
    parts.next()
}

fn parse_query(query: &str) -> Vec<(String, String)> {
    query
        .split('&')
        .filter(|pair| !pair.is_empty())
        .map(|pair| match pair.split_once('=') {
            Some((k, v)) => (k.to_string(), decode(v)),
            None => (pair.to_string(), String::new()),
        })
        .collect()
}

async fn respond(socket: &mut tokio::net::TcpStream, status: &str, body: &str) {
    let html = format!(
        "<!doctype html><html><body style=\"font-family:sans-serif;padding:2rem\">{body}</body></html>"
    );
    let response = format!(
        "HTTP/1.1 {status}\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{html}",
        html.len()
    );
    let _ = socket.write_all(response.as_bytes()).await;
    let _ = socket.flush().await;
}

fn parse_redirect(redirect: &str) -> BotResult<(String, u16, String)> {
    let rest = redirect
        .strip_prefix("http://")
        .or_else(|| redirect.strip_prefix("https://"))
        .ok_or_else(|| BotError::InvalidConfig {
            key: "TWITCH_REDIRECT_URI",
            reason: "must start with http:// or https://".into(),
        })?;

    let (authority, path) = match rest.split_once('/') {
        Some((authority, path)) => (authority, format!("/{path}")),
        None => (rest, "/".to_string()),
    };

    let (host, port) = match authority.split_once(':') {
        Some((host, port)) => (
            host,
            port.parse().map_err(|_| BotError::InvalidConfig {
                key: "TWITCH_REDIRECT_URI",
                reason: "invalid port".into(),
            })?,
        ),
        None => (authority, 80),
    };

    let bind_host = if host == "localhost" { "127.0.0.1" } else { host };
    Ok((bind_host.to_string(), port, path))
}

fn open_browser(url: &str) {
    #[cfg(target_os = "macos")]
    let program = "open";
    #[cfg(target_os = "linux")]
    let program = "xdg-open";
    #[cfg(target_os = "windows")]
    let program = "explorer";

    #[cfg(any(target_os = "macos", target_os = "linux", target_os = "windows"))]
    let _ = std::process::Command::new(program).arg(url).spawn();
}

fn random_state() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    format!("{nanos:032x}")
}

fn encode(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for byte in input.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char)
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

fn decode(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'%' if i + 2 < bytes.len() => {
                let hi = hex_value(bytes[i + 1]);
                let lo = hex_value(bytes[i + 2]);
                if let (Some(hi), Some(lo)) = (hi, lo) {
                    out.push(hi << 4 | lo);
                    i += 3;
                    continue;
                }
                out.push(bytes[i]);
                i += 1;
            }
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            other => {
                out.push(other);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

fn require(key: &'static str) -> BotResult<String> {
    match std::env::var(key) {
        Ok(v) if !v.trim().is_empty() => Ok(v.trim().to_string()),
        _ => Err(BotError::MissingConfig(key)),
    }
}
