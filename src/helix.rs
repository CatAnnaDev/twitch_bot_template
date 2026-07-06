use serde::Deserialize;

use crate::config::Config;
use crate::error::{BotError, BotResult};

const VALIDATE_URL: &str = "https://id.twitch.tv/oauth2/validate";
const TOKEN_URL: &str = "https://id.twitch.tv/oauth2/token";
const HELIX_BASE: &str = "https://api.twitch.tv/helix";

#[derive(Debug, Clone)]
pub struct Helix {
    http: reqwest::Client,
    client_id: String,
    access_token: String,
}

#[allow(dead_code)]
#[derive(Debug, Deserialize)]
pub struct ValidatedToken {
    pub login: String,
    pub user_id: String,
    #[serde(default)]
    pub scopes: Vec<String>,
    #[serde(default)]
    pub expires_in: u64,
}

#[allow(dead_code)]
#[derive(Debug, Deserialize)]
pub struct RefreshedToken {
    pub access_token: String,
    #[serde(default)]
    pub refresh_token: Option<String>,
}

#[allow(dead_code)]
#[derive(Debug, Deserialize)]
pub struct HelixUser {
    pub id: String,
    pub login: String,
    pub display_name: String,
    #[serde(default)]
    pub description: String,
}

#[allow(dead_code)]
#[derive(Debug, Deserialize)]
pub struct HelixStream {
    pub user_login: String,
    pub game_name: String,
    pub title: String,
    pub viewer_count: u64,
    pub started_at: String,
}

#[allow(dead_code)]
#[derive(Debug, Deserialize)]
pub struct ChannelInfo {
    pub broadcaster_login: String,
    pub game_name: String,
    pub title: String,
}

#[derive(Debug, Deserialize)]
struct HelixEnvelope<T> {
    data: Vec<T>,
}

impl Helix {
    pub fn new(config: &Config) -> Self {
        let access_token = config
            .oauth_token
            .strip_prefix("oauth:")
            .unwrap_or(&config.oauth_token)
            .to_string();
        Self {
            http: reqwest::Client::new(),
            client_id: config.client_id.clone(),
            access_token,
        }
    }

    pub fn set_access_token(&mut self, token: String) {
        self.access_token = token.strip_prefix("oauth:").unwrap_or(&token).to_string();
    }

    pub async fn validate(&self) -> BotResult<ValidatedToken> {
        let response = self
            .http
            .get(VALIDATE_URL)
            .header("Authorization", format!("OAuth {}", self.access_token))
            .send()
            .await?;

        if !response.status().is_success() {
            return Err(BotError::Auth(format!(
                "token validation failed with status {}",
                response.status()
            )));
        }

        Ok(response.json().await?)
    }

    pub async fn user_by_login(&self, login: &str) -> BotResult<Option<HelixUser>> {
        let envelope: HelixEnvelope<HelixUser> = self
            .get("users", &[("login", login)])
            .await?;
        Ok(envelope.data.into_iter().next())
    }

    pub async fn last_game(&self, login: &str) -> BotResult<Option<String>> {
        let Some(user) = self.user_by_login(login).await? else {
            return Ok(None);
        };
        let envelope: HelixEnvelope<ChannelInfo> = self
            .get("channels", &[("broadcaster_id", user.id.as_str())])
            .await?;
        Ok(envelope
            .data
            .into_iter()
            .next()
            .map(|c| c.game_name)
            .filter(|g| !g.is_empty()))
    }

    pub async fn stream_by_login(&self, login: &str) -> BotResult<Option<HelixStream>> {
        let envelope: HelixEnvelope<HelixStream> = self
            .get("streams", &[("user_login", login)])
            .await?;
        Ok(envelope.data.into_iter().next())
    }

    pub async fn create_eventsub_subscription(
        &self,
        kind: &str,
        version: &str,
        condition: serde_json::Value,
        session_id: &str,
    ) -> BotResult<()> {
        let body = serde_json::json!({
            "type": kind,
            "version": version,
            "condition": condition,
            "transport": { "method": "websocket", "session_id": session_id },
        });

        let response = self
            .http
            .post(format!("{HELIX_BASE}/eventsub/subscriptions"))
            .header("Client-Id", &self.client_id)
            .header("Authorization", format!("Bearer {}", self.access_token))
            .json(&body)
            .send()
            .await?;

        if response.status().is_success() {
            return Ok(());
        }

        let status = response.status();
        let detail = response.text().await.unwrap_or_default();
        Err(BotError::Auth(format!(
            "eventsub subscription {kind} rejected ({status}): {detail}"
        )))
    }

    async fn get<T: for<'de> Deserialize<'de>>(
        &self,
        path: &str,
        query: &[(&str, &str)],
    ) -> BotResult<T> {
        let response = self
            .http
            .get(format!("{HELIX_BASE}/{path}"))
            .header("Client-Id", &self.client_id)
            .header("Authorization", format!("Bearer {}", self.access_token))
            .query(query)
            .send()
            .await?
            .error_for_status()?;
        Ok(response.json().await?)
    }
}

pub async fn refresh_token(config: &Config) -> BotResult<RefreshedToken> {
    let (Some(secret), Some(refresh)) = (&config.client_secret, &config.refresh_token) else {
        return Err(BotError::Auth(
            "cannot refresh: TWITCH_CLIENT_SECRET and TWITCH_REFRESH_TOKEN are required".into(),
        ));
    };

    let http = reqwest::Client::new();
    let response = http
        .post(TOKEN_URL)
        .form(&[
            ("grant_type", "refresh_token"),
            ("refresh_token", refresh.as_str()),
            ("client_id", config.client_id.as_str()),
            ("client_secret", secret.as_str()),
        ])
        .send()
        .await?;

    if !response.status().is_success() {
        return Err(BotError::Auth(format!(
            "refresh failed with status {}",
            response.status()
        )));
    }

    Ok(response.json().await?)
}
