use serde::Deserialize;
use tokio::sync::mpsc;

use crate::config::Config;
use crate::helix::Helix;
use crate::ws::WebSocket;

const WELCOME_URL: &str = "wss://eventsub.wss.twitch.tv/ws";

#[derive(Debug, Clone)]
pub enum EventSubEvent {
    StreamOnline { broadcaster: String },
    StreamOffline { broadcaster: String },
    Follow { user: String, broadcaster: String },
    Raid {
        from: String,
        from_login: String,
        to: String,
        to_login: String,
        viewers: u64,
    },
    Other { kind: String, data: serde_json::Value },
}

struct Subscription {
    kind: &'static str,
    version: &'static str,
}

const SUBSCRIPTIONS: &[Subscription] = &[
    Subscription { kind: "stream.online", version: "1" },
    Subscription { kind: "stream.offline", version: "1" },
    Subscription { kind: "channel.raid", version: "1" },
    Subscription { kind: "channel.follow", version: "2" },
];

pub async fn run(config: Config, helix: Helix, events: mpsc::Sender<EventSubEvent>) {
    let broadcaster_ids = resolve_broadcaster_ids(&config, &helix).await;
    if broadcaster_ids.is_empty() {
        tracing::warn!("eventsub: no broadcaster ids resolved, disabling eventsub");
        return;
    }

    let bot_user_id = helix
        .validate()
        .await
        .map(|token| token.user_id)
        .unwrap_or_default();

    loop {
        match serve(WELCOME_URL, &helix, &broadcaster_ids, &bot_user_id, &events).await {
            Ok(()) => tracing::warn!("eventsub: session ended, reconnecting in 5s"),
            Err(err) => tracing::error!(%err, "eventsub: session error, reconnecting in 5s"),
        }
        tokio::time::sleep(std::time::Duration::from_secs(5)).await;
    }
}

async fn serve(
    url: &str,
    helix: &Helix,
    broadcaster_ids: &[(String, String)],
    bot_user_id: &str,
    events: &mpsc::Sender<EventSubEvent>,
) -> crate::error::BotResult<()> {
    let mut socket = WebSocket::connect(url).await?;

    let session_id = wait_for_welcome(&mut socket).await?;
    tracing::info!(%session_id, "eventsub: session established");

    for (login, id) in broadcaster_ids {
        subscribe_all(helix, &session_id, id, bot_user_id, login).await;
    }

    loop {
        let Some(text) = socket.read_text().await? else {
            return Ok(());
        };

        let Ok(envelope) = serde_json::from_str::<Envelope>(&text) else {
            continue;
        };

        match envelope.metadata.message_type.as_str() {
            "session_keepalive" => {}
            "session_reconnect" => {
                if let Some(reconnect_url) = envelope
                    .payload
                    .get("session")
                    .and_then(|s| s.get("reconnect_url"))
                    .and_then(|u| u.as_str())
                {
                    tracing::info!("eventsub: reconnect requested");
                    return Box::pin(serve(
                        reconnect_url,
                        helix,
                        broadcaster_ids,
                        bot_user_id,
                        events,
                    ))
                    .await;
                }
            }
            "notification" => {
                if let Some(event) = parse_notification(&envelope) {
                    let _ = events.send(event).await;
                }
            }
            other => tracing::debug!(message_type = %other, "eventsub: ignored message"),
        }
    }
}

async fn wait_for_welcome(socket: &mut WebSocket) -> crate::error::BotResult<String> {
    while let Some(text) = socket.read_text().await? {
        let Ok(envelope) = serde_json::from_str::<Envelope>(&text) else {
            continue;
        };
        if envelope.metadata.message_type == "session_welcome" {
            if let Some(id) = envelope
                .payload
                .get("session")
                .and_then(|s| s.get("id"))
                .and_then(|i| i.as_str())
            {
                return Ok(id.to_string());
            }
        }
    }
    Err(crate::error::BotError::Disconnected)
}

async fn subscribe_all(
    helix: &Helix,
    session_id: &str,
    broadcaster_id: &str,
    bot_user_id: &str,
    login: &str,
) {
    for sub in SUBSCRIPTIONS {
        let condition = match sub.kind {
            "channel.raid" => serde_json::json!({ "to_broadcaster_user_id": broadcaster_id }),
            "channel.follow" => serde_json::json!({
                "broadcaster_user_id": broadcaster_id,
                "moderator_user_id": bot_user_id,
            }),
            _ => serde_json::json!({ "broadcaster_user_id": broadcaster_id }),
        };

        match helix
            .create_eventsub_subscription(sub.kind, sub.version, condition, session_id)
            .await
        {
            Ok(()) => tracing::info!(channel = %login, kind = %sub.kind, "eventsub: subscribed"),
            Err(err) => {
                tracing::warn!(channel = %login, kind = %sub.kind, %err, "eventsub: subscription skipped")
            }
        }
    }
}

async fn resolve_broadcaster_ids(config: &Config, helix: &Helix) -> Vec<(String, String)> {
    let mut ids = Vec::new();
    for channel in &config.channels {
        match helix.user_by_login(channel).await {
            Ok(Some(user)) => ids.push((user.login, user.id)),
            Ok(None) => tracing::warn!(channel = %channel, "eventsub: unknown channel"),
            Err(err) => tracing::warn!(channel = %channel, %err, "eventsub: user lookup failed"),
        }
    }
    ids
}

fn parse_notification(envelope: &Envelope) -> Option<EventSubEvent> {
    let kind = envelope.metadata.subscription_type.as_deref()?;
    let event = envelope.payload.get("event")?;
    let field = |name: &str| event.get(name).and_then(|v| v.as_str()).unwrap_or("").to_string();

    let parsed = match kind {
        "stream.online" => EventSubEvent::StreamOnline {
            broadcaster: field("broadcaster_user_name"),
        },
        "stream.offline" => EventSubEvent::StreamOffline {
            broadcaster: field("broadcaster_user_name"),
        },
        "channel.follow" => EventSubEvent::Follow {
            user: field("user_name"),
            broadcaster: field("broadcaster_user_name"),
        },
        "channel.raid" => EventSubEvent::Raid {
            from: field("from_broadcaster_user_name"),
            from_login: field("from_broadcaster_user_login"),
            to: field("to_broadcaster_user_name"),
            to_login: field("to_broadcaster_user_login"),
            viewers: event.get("viewers").and_then(|v| v.as_u64()).unwrap_or(0),
        },
        other => EventSubEvent::Other {
            kind: other.to_string(),
            data: event.clone(),
        },
    };
    Some(parsed)
}

#[derive(Debug, Deserialize)]
struct Envelope {
    metadata: Metadata,
    payload: serde_json::Value,
}

#[derive(Debug, Deserialize)]
struct Metadata {
    message_type: String,
    #[serde(default)]
    subscription_type: Option<String>,
}
