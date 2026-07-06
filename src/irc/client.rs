use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, ReadHalf, WriteHalf};
use tokio::net::TcpStream;
use tokio::sync::mpsc;
use tokio_rustls::client::TlsStream;

use crate::config::Config;
use crate::error::{BotError, BotResult};
use crate::tls;

const TWITCH_HOST: &str = "irc.chat.twitch.tv";
const TWITCH_PORT: u16 = 6697;

pub struct IrcConnection {
    reader: tokio::io::Lines<BufReader<ReadHalf<TlsStream<TcpStream>>>>,
    out_tx: mpsc::Sender<String>,
}

#[derive(Clone)]
pub struct Outbound {
    tx: mpsc::Sender<String>,
}

impl Outbound {
    pub fn channel(buffer: usize) -> (Self, mpsc::Receiver<String>) {
        let (tx, rx) = mpsc::channel(buffer);
        (Self { tx }, rx)
    }

    pub async fn send_raw(&self, line: impl Into<String>) {
        let _ = self.tx.send(line.into()).await;
    }

    pub async fn say(&self, channel: &str, text: &str) {
        self.send_raw(format!("PRIVMSG #{channel} :{text}")).await;
    }

    #[allow(dead_code)]
    pub async fn reply(&self, parent_id: &str, channel: &str, text: &str) {
        self.send_raw(format!(
            "@reply-parent-msg-id={parent_id} PRIVMSG #{channel} :{text}"
        ))
        .await;
    }
}

impl IrcConnection {
    pub async fn connect(config: &Config) -> BotResult<Self> {
        let stream = tls::connect(TWITCH_HOST, TWITCH_PORT).await?;

        let (read_half, mut write_half) = tokio::io::split(stream);
        let reader = BufReader::new(read_half).lines();

        write_handshake(&mut write_half, config).await?;

        let (out_tx, out_rx) = mpsc::channel::<String>(256);
        spawn_writer(write_half, out_rx);

        Ok(Self { reader, out_tx })
    }

    pub fn outbound(&self) -> Outbound {
        Outbound {
            tx: self.out_tx.clone(),
        }
    }

    pub async fn next_line(&mut self) -> BotResult<String> {
        match self.reader.next_line().await? {
            Some(line) => Ok(line),
            None => Err(BotError::Disconnected),
        }
    }
}

async fn write_handshake(
    write_half: &mut WriteHalf<TlsStream<TcpStream>>,
    config: &Config,
) -> BotResult<()> {
    let caps = "CAP REQ :twitch.tv/tags twitch.tv/commands twitch.tv/membership\r\n";
    write_half.write_all(caps.as_bytes()).await?;
    write_half
        .write_all(format!("PASS {}\r\n", config.oauth_token).as_bytes())
        .await?;
    write_half
        .write_all(format!("NICK {}\r\n", config.bot_username).as_bytes())
        .await?;
    for channel in &config.channels {
        write_half
            .write_all(format!("JOIN #{channel}\r\n").as_bytes())
            .await?;
    }
    write_half.flush().await?;
    Ok(())
}

fn spawn_writer(mut write_half: WriteHalf<TlsStream<TcpStream>>, mut rx: mpsc::Receiver<String>) {
    tokio::spawn(async move {
        while let Some(line) = rx.recv().await {
            if write_half.write_all(line.as_bytes()).await.is_err() {
                break;
            }
            if write_half.write_all(b"\r\n").await.is_err() {
                break;
            }
            if write_half.flush().await.is_err() {
                break;
            }
        }
    });
}

