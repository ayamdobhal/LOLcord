use serde::{Deserialize, Serialize};

/// Client → Server messages
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ClientMessage {
    Join {
        username: String,
        room: String,
        #[serde(default)]
        password: Option<String>,
    },
    Leave,
    Chat {
        text: String,
    },
}

/// Server → Client messages
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ServerMessage {
    RoomState {
        room: String,
        users: Vec<String>,
    },
    UserJoined {
        username: String,
    },
    UserLeft {
        username: String,
    },
    Chat {
        from: String,
        text: String,
        ts: u64,
    },
    Error {
        message: String,
    },
}

/// Length-prefixed protocol helpers.
/// Wire format: [4-byte big-endian length][JSON payload]
pub mod protocol {
    use anyhow::{Context, Result, bail};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    /// Write a length-prefixed JSON message.
    pub async fn write_message<W, T>(writer: &mut W, msg: &T) -> Result<()>
    where
        W: AsyncWriteExt + Unpin,
        T: serde::Serialize,
    {
        let payload = serde_json::to_vec(msg)?;
        let len = payload.len() as u32;
        writer.write_all(&len.to_be_bytes()).await?;
        writer.write_all(&payload).await?;
        writer.flush().await?;
        Ok(())
    }

    /// Read a length-prefixed JSON message.
    pub async fn read_message<R, T>(reader: &mut R) -> Result<T>
    where
        R: AsyncReadExt + Unpin,
        T: serde::de::DeserializeOwned,
    {
        let mut len_buf = [0u8; 4];
        reader
            .read_exact(&mut len_buf)
            .await
            .context("connection closed")?;
        let len = u32::from_be_bytes(len_buf) as usize;

        if len > 1024 * 64 {
            bail!("message too large: {len} bytes");
        }

        let mut buf = vec![0u8; len];
        reader.read_exact(&mut buf).await?;
        let msg = serde_json::from_slice(&buf)?;
        Ok(msg)
    }
}

pub const MAX_ROOM_USERS: usize = 10;
pub const DEFAULT_PORT: u16 = 7480;
