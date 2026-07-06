mod bot;
mod commands;
mod config;
mod db;
mod error;
mod eventsub;
mod feed;
mod helix;
mod irc;
mod oauth;
mod timers;
mod tls;
mod ws;

#[cfg(feature = "gui")]
mod gui;

use config::Config;
use db::Db;
use error::BotResult;
use feed::Feed;
use helix::Helix;
use irc::Outbound;

fn main() {
    init_tracing();

    let result = match std::env::args().nth(1).as_deref() {
        Some("auth") => run_auth(),
        _ => run(),
    };

    if let Err(err) = result {
        tracing::error!(%err, "fatal error");
        std::process::exit(1);
    }
}

fn run_auth() -> BotResult<()> {
    let runtime = tokio::runtime::Runtime::new()?;
    runtime.block_on(oauth::run())
}

#[cfg(not(feature = "gui"))]
fn run() -> BotResult<()> {
    let runtime = tokio::runtime::Runtime::new()?;
    runtime.block_on(async {
        let (config, db, helix) = bootstrap().await?;
        let (out, chat_rx) = Outbound::channel(256);
        spawn_side_tasks(&config, &helix, out, Feed::default());
        let bot = bot::Bot::new(config, db, helix, chat_rx);
        bot.run().await
    })
}

#[cfg(feature = "gui")]
fn run() -> BotResult<()> {
    let runtime = tokio::runtime::Runtime::new()?;
    let (config, db, helix) = runtime.block_on(bootstrap())?;

    let _guard = runtime.enter();
    let feed = Feed::default();
    let (out, chat_rx) = Outbound::channel(256);
    spawn_side_tasks(&config, &helix, out.clone(), feed.clone());

    let bot_config = config.clone();
    let bot_db = db.clone();
    let bot_helix = helix.clone();
    runtime.spawn(async move {
        let bot = bot::Bot::new(bot_config, bot_db, bot_helix, chat_rx);
        if let Err(err) = bot.run().await {
            tracing::error!(%err, "bot loop exited");
        }
    });

    gui::launch(gui::GuiContext {
        config,
        db,
        handle: runtime.handle().clone(),
        out,
        feed,
    });
    Ok(())
}

async fn bootstrap() -> BotResult<(Config, Db, Helix)> {
    let config = Config::load()?;

    let mut helix = Helix::new(&config);
    match helix.validate().await {
        Ok(token) => tracing::info!(
            login = %token.login,
            user_id = %token.user_id,
            expires_in = token.expires_in,
            "token validated"
        ),
        Err(err) => {
            tracing::warn!(%err, "token validation failed, attempting refresh");
            let refreshed = helix::refresh_token(&config).await?;
            helix.set_access_token(refreshed.access_token.clone());
            helix.validate().await?;
            tracing::info!("token refreshed and validated");
        }
    }

    let db = Db::connect(&config).await?;
    tracing::info!(database = %config.database_url, "database ready");

    Ok((config, db, helix))
}

fn spawn_side_tasks(config: &Config, helix: &Helix, out: Outbound, feed: Feed) {
    use eventsub::EventSubEvent;

    let (tx, mut rx) = tokio::sync::mpsc::channel::<EventSubEvent>(64);
    tokio::spawn(eventsub::run(config.clone(), helix.clone(), tx));

    let consumer_helix = helix.clone();
    let consumer_out = out.clone();
    let consumer_feed = feed.clone();
    tokio::spawn(async move {
        while let Some(event) = rx.recv().await {
            match event {
                EventSubEvent::StreamOnline { broadcaster } => {
                    tracing::info!(%broadcaster, "eventsub: stream online");
                    consumer_feed.push(format!("[online] {broadcaster} went live"));
                }
                EventSubEvent::StreamOffline { broadcaster } => {
                    tracing::info!(%broadcaster, "eventsub: stream offline");
                    consumer_feed.push(format!("[offline] {broadcaster} stopped streaming"));
                }
                EventSubEvent::Follow { user, broadcaster } => {
                    tracing::info!(%user, %broadcaster, "eventsub: new follower");
                    consumer_feed.push(format!("[follow] {user} followed {broadcaster}"));
                }
                EventSubEvent::Raid {
                    from,
                    from_login,
                    to,
                    to_login,
                    viewers,
                } => {
                    tracing::info!(%from, %to, viewers, "eventsub: incoming raid");
                    consumer_feed.push(format!("[raid] {from} -> {to} ({viewers} viewers)"));
                    let game = consumer_helix.last_game(&from_login).await.ok().flatten();
                    let mut message = format!(
                        "Raid incoming! Shout-out to @{from} who brought {viewers} viewers. Go follow them at twitch.tv/{from_login}"
                    );
                    match game {
                        Some(game) => message.push_str(&format!(" - last streaming {game}.")),
                        None => message.push('.'),
                    }
                    consumer_out.say(&to_login, &message).await;
                }
                EventSubEvent::Other { kind, data } => {
                    tracing::info!(%kind, %data, "eventsub: event");
                    consumer_feed.push(format!("[{kind}] {data}"));
                }
            }
        }
    });

    timers::spawn(config.clone(), out);
}

fn init_tracing() {
    use tracing_subscriber::EnvFilter;
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| EnvFilter::new("twitch_bot=info,warn")),
        )
        .init();
}
