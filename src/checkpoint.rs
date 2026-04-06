use std::collections::HashSet;

use anyhow::Result;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::{models::OwnedMessage, storage};

pub const SAVE_INTERVAL: usize = 1_000;

#[derive(Serialize, Deserialize)]
pub struct ChannelCheckpoint {
    pub channel_id: String,
    pub cutoff_key: String,
    pub found: Vec<OwnedMessage>,
    pub deleted_ids: HashSet<String>,
}

impl ChannelCheckpoint {
    pub fn new(channel_id: &str, cutoff_key: &str, found: Vec<OwnedMessage>) -> Self {
        Self {
            channel_id: channel_id.to_string(),
            cutoff_key: cutoff_key.to_string(),
            found,
            deleted_ids: HashSet::new(),
        }
    }

    pub fn pending(&self) -> Vec<OwnedMessage> {
        self.found
            .iter()
            .filter(|message| !self.deleted_ids.contains(&message.id))
            .cloned()
            .collect()
    }

    pub fn mark_deleted(&mut self, id: &str) {
        self.deleted_ids.insert(id.to_string());
    }

    pub fn save(&self) -> Result<()> {
        let deleted_ids = self.deleted_ids.iter().cloned().collect::<Vec<_>>();
        storage::upsert_checkpoint(
            &self.channel_id,
            &self.cutoff_key,
            &self.found,
            &deleted_ids,
        )
    }

    pub fn remove(&self) {
        let _ = storage::remove_checkpoint(&self.channel_id, &self.cutoff_key);
    }
}

pub fn load(channel_id: &str, cutoff_key: &str) -> Option<ChannelCheckpoint> {
    let (found, deleted_ids) = storage::load_checkpoint(channel_id, cutoff_key).ok()??;
    Some(ChannelCheckpoint {
        channel_id: channel_id.to_string(),
        cutoff_key: cutoff_key.to_string(),
        found,
        deleted_ids: deleted_ids.into_iter().collect(),
    })
}

pub fn cutoff_key(cutoff: Option<DateTime<Utc>>) -> String {
    match cutoff {
        Some(dt) => dt.timestamp().to_string(),
        None => "all".to_string(),
    }
}

pub fn clear_all() -> Result<()> {
    storage::clear_checkpoints()
}
