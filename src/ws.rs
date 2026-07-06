use std::time::{SystemTime, UNIX_EPOCH};

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio_rustls::client::TlsStream;

use crate::error::{BotError, BotResult};
use crate::tls;

const OP_CONTINUATION: u8 = 0x0;
const OP_TEXT: u8 = 0x1;
const OP_BINARY: u8 = 0x2;
const OP_CLOSE: u8 = 0x8;
const OP_PING: u8 = 0x9;
const OP_PONG: u8 = 0xA;

pub struct WebSocket {
    stream: TlsStream<TcpStream>,
    mask_state: u32,
}

impl WebSocket {
    pub async fn connect(url: &str) -> BotResult<Self> {
        let (host, port, path) = parse_url(url)?;
        let mut stream = tls::connect(&host, port).await?;
        handshake(&mut stream, &host, &path).await?;
        Ok(Self {
            stream,
            mask_state: seed(),
        })
    }

    pub async fn read_text(&mut self) -> BotResult<Option<String>> {
        let mut buffer: Vec<u8> = Vec::new();
        loop {
            let frame = self.read_frame().await?;
            match frame.opcode {
                OP_TEXT | OP_BINARY | OP_CONTINUATION => {
                    buffer.extend_from_slice(&frame.payload);
                    if frame.fin {
                        return Ok(Some(String::from_utf8_lossy(&buffer).into_owned()));
                    }
                }
                OP_PING => self.write_frame(OP_PONG, &frame.payload).await?,
                OP_PONG => {}
                OP_CLOSE => {
                    let _ = self.write_frame(OP_CLOSE, &frame.payload).await;
                    return Ok(None);
                }
                _ => {}
            }
        }
    }

    async fn read_frame(&mut self) -> BotResult<Frame> {
        let mut header = [0u8; 2];
        self.stream.read_exact(&mut header).await?;

        let fin = header[0] & 0x80 != 0;
        let opcode = header[0] & 0x0f;
        let masked = header[1] & 0x80 != 0;

        let mut len = (header[1] & 0x7f) as usize;
        if len == 126 {
            let mut ext = [0u8; 2];
            self.stream.read_exact(&mut ext).await?;
            len = u16::from_be_bytes(ext) as usize;
        } else if len == 127 {
            let mut ext = [0u8; 8];
            self.stream.read_exact(&mut ext).await?;
            len = u64::from_be_bytes(ext) as usize;
        }

        let mut mask = [0u8; 4];
        if masked {
            self.stream.read_exact(&mut mask).await?;
        }

        let mut payload = vec![0u8; len];
        self.stream.read_exact(&mut payload).await?;
        if masked {
            for (i, byte) in payload.iter_mut().enumerate() {
                *byte ^= mask[i % 4];
            }
        }

        Ok(Frame {
            fin,
            opcode,
            payload,
        })
    }

    async fn write_frame(&mut self, opcode: u8, payload: &[u8]) -> BotResult<()> {
        let mut frame = Vec::with_capacity(payload.len() + 14);
        frame.push(0x80 | opcode);

        let len = payload.len();
        if len < 126 {
            frame.push(0x80 | len as u8);
        } else if len <= u16::MAX as usize {
            frame.push(0x80 | 126);
            frame.extend_from_slice(&(len as u16).to_be_bytes());
        } else {
            frame.push(0x80 | 127);
            frame.extend_from_slice(&(len as u64).to_be_bytes());
        }

        let mask = self.next_mask();
        frame.extend_from_slice(&mask);
        for (i, byte) in payload.iter().enumerate() {
            frame.push(byte ^ mask[i % 4]);
        }

        self.stream.write_all(&frame).await?;
        self.stream.flush().await?;
        Ok(())
    }

    fn next_mask(&mut self) -> [u8; 4] {
        let mut state = self.mask_state;
        state ^= state << 13;
        state ^= state >> 17;
        state ^= state << 5;
        self.mask_state = state;
        state.to_be_bytes()
    }
}

struct Frame {
    fin: bool,
    opcode: u8,
    payload: Vec<u8>,
}

fn parse_url(url: &str) -> BotResult<(String, u16, String)> {
    let rest = url
        .strip_prefix("wss://")
        .or_else(|| url.strip_prefix("https://"))
        .ok_or_else(|| BotError::Tls(format!("unsupported websocket url: {url}")))?;

    let (authority, path) = match rest.split_once('/') {
        Some((authority, path)) => (authority, format!("/{path}")),
        None => (rest, "/".to_string()),
    };

    let (host, port) = match authority.split_once(':') {
        Some((host, port)) => (
            host.to_string(),
            port.parse().map_err(|_| {
                BotError::Tls(format!("invalid port in websocket url: {authority}"))
            })?,
        ),
        None => (authority.to_string(), 443),
    };

    Ok((host, port, path))
}

async fn handshake(stream: &mut TlsStream<TcpStream>, host: &str, path: &str) -> BotResult<()> {
    let key = base64(&seed().to_be_bytes().repeat(4)[..16]);
    let request = format!(
        "GET {path} HTTP/1.1\r\n\
         Host: {host}\r\n\
         Upgrade: websocket\r\n\
         Connection: Upgrade\r\n\
         Sec-WebSocket-Key: {key}\r\n\
         Sec-WebSocket-Version: 13\r\n\r\n"
    );
    stream.write_all(request.as_bytes()).await?;
    stream.flush().await?;

    let mut response = Vec::new();
    let mut byte = [0u8; 1];
    while !response.ends_with(b"\r\n\r\n") {
        let read = stream.read(&mut byte).await?;
        if read == 0 {
            return Err(BotError::Disconnected);
        }
        response.push(byte[0]);
        if response.len() > 8192 {
            return Err(BotError::Tls("handshake response too large".into()));
        }
    }

    let head = String::from_utf8_lossy(&response);
    if !head.starts_with("HTTP/1.1 101") {
        let status = head.lines().next().unwrap_or("").to_string();
        return Err(BotError::Tls(format!("websocket upgrade failed: {status}")));
    }
    Ok(())
}

fn seed() -> u32 {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0x9e37_79b9);
    nanos | 1
}

fn base64(input: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity((input.len() + 2) / 3 * 4);
    for chunk in input.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = *chunk.get(1).unwrap_or(&0) as u32;
        let b2 = *chunk.get(2).unwrap_or(&0) as u32;
        let triple = (b0 << 16) | (b1 << 8) | b2;
        out.push(TABLE[(triple >> 18 & 0x3f) as usize] as char);
        out.push(TABLE[(triple >> 12 & 0x3f) as usize] as char);
        out.push(if chunk.len() > 1 {
            TABLE[(triple >> 6 & 0x3f) as usize] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            TABLE[(triple & 0x3f) as usize] as char
        } else {
            '='
        });
    }
    out
}
