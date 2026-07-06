use std::path::Path;

use crate::error::{BotError, BotResult};

#[derive(Debug, Clone)]
pub struct Config {
    pub bot_username: String,
    pub oauth_token: String,
    pub client_id: String,
    pub client_secret: Option<String>,
    pub refresh_token: Option<String>,
    pub channels: Vec<String>,
    pub command_prefix: char,
    pub database_url: String,
}

impl Config {
    pub fn load() -> BotResult<Self> {
        load_dotenv(".env");

        let bot_username = require("TWITCH_BOT_USERNAME")?.to_ascii_lowercase();
        let oauth_token = normalize_token(&require("TWITCH_OAUTH_TOKEN")?);
        let client_id = require("TWITCH_CLIENT_ID")?;

        let client_secret = optional("TWITCH_CLIENT_SECRET");
        let refresh_token = optional("TWITCH_REFRESH_TOKEN");

        let channels: Vec<String> = require("TWITCH_CHANNELS")?
            .split(',')
            .map(|c| c.trim().trim_start_matches('#').to_ascii_lowercase())
            .filter(|c| !c.is_empty())
            .collect();

        if channels.is_empty() {
            return Err(BotError::InvalidConfig {
                key: "TWITCH_CHANNELS",
                reason: "no channel names found".into(),
            });
        }

        let command_prefix = optional("COMMAND_PREFIX")
            .and_then(|p| p.chars().next())
            .unwrap_or('!');

        let database_url =
            optional("DATABASE_URL").unwrap_or_else(|| "sqlite:twitch_bot.db?mode=rwc".into());

        Ok(Self {
            bot_username,
            oauth_token,
            client_id,
            client_secret,
            refresh_token,
            channels,
            command_prefix,
            database_url,
        })
    }
}

fn require(key: &'static str) -> BotResult<String> {
    match std::env::var(key) {
        Ok(v) if !v.trim().is_empty() => Ok(v.trim().to_string()),
        _ => Err(BotError::MissingConfig(key)),
    }
}

fn optional(key: &str) -> Option<String> {
    std::env::var(key)
        .ok()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
}

fn normalize_token(raw: &str) -> String {
    let stripped = raw.trim().strip_prefix("oauth:").unwrap_or(raw.trim());
    format!("oauth:{stripped}")
}

pub fn load_dotenv(path: impl AsRef<Path>) {
    let Ok(contents) = std::fs::read_to_string(path) else {
        return;
    };

    for line in contents.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let key = key.trim();
        let value = value.trim().trim_matches(|c| c == '"' || c == '\'');
        if std::env::var_os(key).is_none() {
            unsafe { std::env::set_var(key, value) };
        }
    }
}
