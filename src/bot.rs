use tokio::sync::mpsc;

use crate::commands::{self, CommandContext};
use crate::config::Config;
use crate::db::Db;
use crate::helix::Helix;
use crate::irc::{IrcConnection, IrcMessage, Outbound};

pub struct Bot {
    config: Config,
    db: Db,
    helix: Helix,
    chat_rx: mpsc::Receiver<String>,
}

impl Bot {
    pub fn new(config: Config, db: Db, helix: Helix, chat_rx: mpsc::Receiver<String>) -> Self {
        Self {
            config,
            db,
            helix,
            chat_rx,
        }
    }

    pub async fn run(mut self) -> crate::error::BotResult<()> {
        loop {
            match self.connect_and_serve().await {
                Ok(()) => {
                    tracing::warn!("connection closed cleanly, reconnecting in 3s");
                }
                Err(err) => {
                    tracing::error!(%err, "connection error, reconnecting in 3s");
                }
            }
            tokio::time::sleep(std::time::Duration::from_secs(3)).await;
        }
    }

    async fn connect_and_serve(&mut self) -> crate::error::BotResult<()> {
        let mut conn = IrcConnection::connect(&self.config).await?;
        let out = conn.outbound();
        tracing::info!(
            channels = ?self.config.channels,
            username = %self.config.bot_username,
            "connected to twitch chat"
        );

        loop {
            tokio::select! {
                line = conn.next_line() => {
                    let Some(msg) = IrcMessage::parse(&line?) else {
                        continue;
                    };
                    self.handle(&msg, &out).await;
                }
                Some(chat_line) = self.chat_rx.recv() => {
                    out.send_raw(chat_line).await;
                }
            }
        }
    }

    async fn handle(&self, msg: &IrcMessage, out: &Outbound) {
        match msg.command.as_str() {
            "PING" => {
                let token = msg.params.first().map(String::as_str).unwrap_or("tmi.twitch.tv");
                out.send_raw(format!("PONG :{token}")).await;
            }
            "PRIVMSG" => self.handle_privmsg(msg, out).await,
            "RECONNECT" => {
                tracing::info!("twitch requested reconnect");
            }
            _ => {}
        }
    }

    async fn handle_privmsg(&self, msg: &IrcMessage, out: &Outbound) {
        let (Some(channel), Some(text)) = (msg.channel(), msg.text()) else {
            return;
        };
        let login = msg.sender_login().unwrap_or("unknown");

        if let Err(err) = self.db.log_message(channel, login, text).await {
            tracing::warn!(%err, "failed to log message");
        }

        let Some(rest) = text.strip_prefix(self.config.command_prefix) else {
            return;
        };
        let mut parts = rest.split_whitespace();
        let Some(name) = parts.next() else {
            return;
        };
        let args: Vec<&str> = parts.collect();

        tracing::debug!(%channel, %login, command = %name, "dispatching command");

        commands::dispatch(CommandContext {
            name: &name.to_ascii_lowercase(),
            args,
            channel,
            message: msg,
            db: &self.db,
            helix: &self.helix,
            out,
        })
        .await;
    }
}
