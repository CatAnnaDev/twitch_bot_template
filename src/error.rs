use thiserror::Error;

#[derive(Debug, Error)]
pub enum BotError {
    #[error("missing required setting: {0}")]
    MissingConfig(&'static str),

    #[error("invalid setting {key}: {reason}")]
    InvalidConfig { key: &'static str, reason: String },

    #[error("twitch rejected authentication: {0}")]
    Auth(String),

    #[error("i/o error: {0}")]
    Io(#[from] std::io::Error),

    #[error("tls error: {0}")]
    Tls(String),

    #[error("http error: {0}")]
    Http(#[from] reqwest::Error),

    #[error("database error: {0}")]
    Database(#[from] sqlx::Error),

    #[error("migration error: {0}")]
    Migrate(#[from] sqlx::migrate::MigrateError),

    #[error("connection closed by peer")]
    Disconnected,
}

pub type BotResult<T> = Result<T, BotError>;
