use anyhow::{bail, Result};
use shared::ServerMessage;
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicU16, Ordering};
use tokio::sync::{RwLock, mpsc};
use tracing::info;

static NEXT_ROOM_ID: AtomicU16 = AtomicU16::new(1);
static NEXT_USER_ID: AtomicU16 = AtomicU16::new(1);

pub struct Member {
    pub tx: mpsc::UnboundedSender<ServerMessage>,
    pub user_id: u16,
    /// Set once the client sends its first UDP packet
    pub voice_addr: Option<SocketAddr>,
}

struct Room {
    room_id: u16,
    password: Option<String>,
    members: HashMap<String, Member>,
}

impl Room {
    fn new(room_id: u16, password: Option<String>) -> Self {
        Self {
            room_id,
            password,
            members: HashMap::new(),
        }
    }
}

pub struct RoomManager {
    rooms: RwLock<HashMap<String, Room>>,
    max_users_per_room: usize,
    max_rooms: usize,
}

impl RoomManager {
    pub fn new(max_users_per_room: usize, max_rooms: usize) -> Self {
        Self {
            rooms: RwLock::new(HashMap::new()),
            max_users_per_room,
            max_rooms,
        }
    }

    /// Join a room. Returns (existing usernames, assigned user_id, room_id, user_id_map).
    pub async fn join(
        &self,
        room_name: &str,
        username: &str,
        password: Option<&str>,
    ) -> Result<(Vec<String>, u16, u16, HashMap<String, u16>)> {
        let mut rooms = self.rooms.write().await;

        // Check max rooms before creating a new one
        if !rooms.contains_key(room_name) && rooms.len() >= self.max_rooms {
            bail!("server room limit reached (max {})", self.max_rooms);
        }

        let room = rooms
            .entry(room_name.to_string())
            .or_insert_with(|| {
                let rid = NEXT_ROOM_ID.fetch_add(1, Ordering::Relaxed);
                Room::new(rid, password.map(|s| s.to_string()))
            });

        if let Some(ref room_pass) = room.password {
            match password {
                Some(p) if p == room_pass => {}
                _ => bail!("incorrect room password"),
            }
        }

        if room.members.len() >= self.max_users_per_room {
            bail!("room is full (max {})", self.max_users_per_room);
        }

        if room.members.contains_key(username) {
            bail!("username '{username}' already taken in this room");
        }

        let users: Vec<String> = room.members.keys().cloned().collect();
        let user_ids: HashMap<String, u16> = room.members.iter()
            .map(|(name, member)| (name.clone(), member.user_id))
            .collect();
        let user_id = NEXT_USER_ID.fetch_add(1, Ordering::Relaxed);
        let room_id = room.room_id;

        info!(
            "{username} joined room '{room_name}' (id={room_id}, uid={user_id}, {} users)",
            users.len() + 1
        );

        Ok((users, user_id, room_id, user_ids))
    }

    pub async fn subscribe(
        &self,
        room_name: &str,
        username: &str,
        user_id: u16,
        tx: mpsc::UnboundedSender<ServerMessage>,
    ) {
        let mut rooms = self.rooms.write().await;
        if let Some(room) = rooms.get_mut(room_name) {
            room.members.insert(
                username.to_string(),
                Member {
                    tx,
                    user_id,
                    voice_addr: None,
                },
            );
        }
    }

    pub async fn leave(&self, room_name: &str, username: &str) {
        let mut rooms = self.rooms.write().await;
        if let Some(room) = rooms.get_mut(room_name) {
            room.members.remove(username);
            info!(
                "{username} left room '{room_name}' ({} users)",
                room.members.len()
            );
            if room.members.is_empty() {
                rooms.remove(room_name);
                info!("room '{room_name}' removed (empty)");
            }
        }
    }

    pub async fn broadcast(&self, room_name: &str, sender: &str, msg: ServerMessage) {
        let rooms = self.rooms.read().await;
        if let Some(room) = rooms.get(room_name) {
            for (name, member) in &room.members {
                if name != sender {
                    let _ = member.tx.send(msg.clone());
                }
            }
        }
    }

    /// Register a voice address for a user (called on first UDP packet).
    pub async fn register_voice_addr(
        &self,
        room_id: u16,
        user_id: u16,
        addr: SocketAddr,
    ) -> bool {
        let mut rooms = self.rooms.write().await;
        for room in rooms.values_mut() {
            if room.room_id == room_id {
                for member in room.members.values_mut() {
                    if member.user_id == user_id {
                        member.voice_addr = Some(addr);
                        return true;
                    }
                }
            }
        }
        false
    }

    /// Get all voice addresses in a room except the sender.
    pub async fn get_voice_peers(
        &self,
        room_id: u16,
        sender_user_id: u16,
    ) -> Vec<SocketAddr> {
        let rooms = self.rooms.read().await;
        for room in rooms.values() {
            if room.room_id == room_id {
                return room
                    .members
                    .values()
                    .filter(|m| m.user_id != sender_user_id && m.voice_addr.is_some())
                    .map(|m| m.voice_addr.unwrap())
                    .collect();
            }
        }
        Vec::new()
    }
}
