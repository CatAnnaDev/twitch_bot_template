use sqlx::sqlite::{SqlitePool, SqlitePoolOptions};

use crate::config::Config;
use crate::error::BotResult;

#[derive(Debug, Clone)]
pub struct Db {
    pool: SqlitePool,
}

#[allow(dead_code)]
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct CustomCommand {
    pub name: String,
    pub response: String,
    pub uses: i64,
}

#[allow(dead_code)]
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct MessageRow {
    pub channel: String,
    pub login: String,
    pub text: String,
    pub at: String,
}

impl Db {
    pub async fn connect(config: &Config) -> BotResult<Self> {
        let pool = SqlitePoolOptions::new()
            .max_connections(5)
            .connect(&config.database_url)
            .await?;

        sqlx::migrate!("./migrations").run(&pool).await?;

        Ok(Self { pool })
    }

    pub async fn log_message(&self, channel: &str, login: &str, text: &str) -> BotResult<()> {
        sqlx::query("INSERT INTO messages (channel, login, text) VALUES (?, ?, ?)")
            .bind(channel)
            .bind(login)
            .bind(text)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    pub async fn custom_command(&self, name: &str) -> BotResult<Option<CustomCommand>> {
        let row = sqlx::query_as::<_, CustomCommand>(
            "SELECT name, response, uses FROM commands WHERE name = ?",
        )
        .bind(name)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row)
    }

    pub async fn all_commands(&self) -> BotResult<Vec<CustomCommand>> {
        let rows = sqlx::query_as::<_, CustomCommand>(
            "SELECT name, response, uses FROM commands ORDER BY name",
        )
        .fetch_all(&self.pool)
        .await?;
        Ok(rows)
    }

    #[cfg_attr(not(feature = "gui"), allow(dead_code))]
    pub async fn recent_messages(&self, limit: i64) -> BotResult<Vec<MessageRow>> {
        let rows = sqlx::query_as::<_, MessageRow>(
            "SELECT channel, login, text, at FROM messages ORDER BY id DESC LIMIT ?",
        )
        .bind(limit)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows)
    }

    pub async fn upsert_command(&self, name: &str, response: &str) -> BotResult<()> {
        sqlx::query(
            "INSERT INTO commands (name, response, uses) VALUES (?, ?, 0)
             ON CONFLICT(name) DO UPDATE SET response = excluded.response",
        )
        .bind(name)
        .bind(response)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn delete_command(&self, name: &str) -> BotResult<bool> {
        let result = sqlx::query("DELETE FROM commands WHERE name = ?")
            .bind(name)
            .execute(&self.pool)
            .await?;
        Ok(result.rows_affected() > 0)
    }

    pub async fn bump_command_use(&self, name: &str) -> BotResult<()> {
        sqlx::query("UPDATE commands SET uses = uses + 1 WHERE name = ?")
            .bind(name)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    pub async fn message_count(&self, channel: &str) -> BotResult<i64> {
        let (count,): (i64,) =
            sqlx::query_as("SELECT COUNT(*) FROM messages WHERE channel = ?")
                .bind(channel)
                .fetch_one(&self.pool)
                .await?;
        Ok(count)
    }
}
