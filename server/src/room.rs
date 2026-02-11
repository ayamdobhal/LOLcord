use anyhow::{bail, Result};
use shared::{ServerMessage, MAX_ROOM_USERS};
use std::collections::HashMap;
use tokio::sync::{RwLock, mpsc};
use tracing::info;

struct Room {
    password: Option<String>,
    /// username → sender channel
    members: HashMap<String, mpsc::UnboundedSender<ServerMessage>>,
}

impl Room {
    fn new(password: Option<String>) -> Self {
        Self {
            password,
            members: HashMap::new(),
        }
    }
}

pub struct RoomManager {
    rooms: RwLock<HashMap<String, Room>>,
}

impl RoomManager {
    pub fn new() -> Self {
        Self {
            rooms: RwLock::new(HashMap::new()),
        }
    }

    /// Join a room. Creates it if it doesn't exist. Returns list of current usernames.
    pub async fn join(
        &self,
        room_name: &str,
        username: &str,
        password: Option<&str>,
    ) -> Result<Vec<String>> {
        let mut rooms = self.rooms.write().await;
        let room = rooms
            .entry(room_name.to_string())
            .or_insert_with(|| Room::new(password.map(|s| s.to_string())));

        // Check password
        if let Some(ref room_pass) = room.password {
            match password {
                Some(p) if p == room_pass => {}
                _ => bail!("incorrect room password"),
            }
        }

        // Check capacity
        if room.members.len() >= MAX_ROOM_USERS {
            bail!("room is full (max {MAX_ROOM_USERS})");
        }

        // Check duplicate username
        if room.members.contains_key(username) {
            bail!("username '{username}' already taken in this room");
        }

        let users: Vec<String> = room.members.keys().cloned().collect();
        info!("{username} joined room '{room_name}' ({} users)", users.len() + 1);
        Ok(users)
    }

    /// Register the sender channel for a user (called after join succeeds).
    pub async fn subscribe(
        &self,
        room_name: &str,
        username: &str,
        tx: mpsc::UnboundedSender<ServerMessage>,
    ) {
        let mut rooms = self.rooms.write().await;
        if let Some(room) = rooms.get_mut(room_name) {
            room.members.insert(username.to_string(), tx);
        }
    }

    /// Remove a user from a room. Cleans up empty rooms.
    pub async fn leave(&self, room_name: &str, username: &str) {
        let mut rooms = self.rooms.write().await;
        if let Some(room) = rooms.get_mut(room_name) {
            room.members.remove(username);
            info!("{username} left room '{room_name}' ({} users)", room.members.len());
            if room.members.is_empty() {
                rooms.remove(room_name);
                info!("room '{room_name}' removed (empty)");
            }
        }
    }

    /// Broadcast a message to all members of a room except the sender.
    pub async fn broadcast(&self, room_name: &str, sender: &str, msg: ServerMessage) {
        let rooms = self.rooms.read().await;
        if let Some(room) = rooms.get(room_name) {
            for (name, tx) in &room.members {
                if name != sender {
                    let _ = tx.send(msg.clone());
                }
            }
        }
    }
}
