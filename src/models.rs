use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Deserialize, Serialize, Debug, Clone)]
pub struct Me {
    pub id: String,
    pub username: String,
}

#[derive(Deserialize, Serialize, Debug, Clone)]
pub struct Guild {
    #[serde(default)]
    pub id: String,
    pub name: String,
}

#[derive(Deserialize, Serialize, Debug, Clone)]
pub struct Channel {
    pub id: String,
    pub name: Option<String>,
    pub guild_id: Option<String>,
    #[serde(default)]
    pub last_message_id: Option<String>,
    #[serde(default)]
    pub recipients: Vec<User>,
    #[serde(rename = "type")]
    pub kind: u8,
}

#[derive(Deserialize, Serialize, Debug, Clone)]
pub struct User {
    pub id: String,
    pub username: String,
    #[serde(default)]
    pub global_name: Option<String>,
}

#[derive(Deserialize, Debug)]
pub struct Message {
    pub id: String,
    #[serde(default)]
    pub channel_id: Option<String>,
    #[serde(default)]
    pub guild_id: Option<String>,
    pub author: Author,
    pub timestamp: DateTime<Utc>,
    #[serde(default)]
    pub content: String,
    #[serde(default)]
    pub attachments: Vec<Attachment>,
}

#[derive(Deserialize, Debug, Clone)]
pub struct Attachment {
    #[serde(default)]
    pub filename: Option<String>,
    #[serde(default)]
    pub content_type: Option<String>,
}

#[derive(Deserialize, Debug)]
pub struct Author {
    pub id: String,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct OwnedMessage {
    pub id: String,
    pub timestamp: DateTime<Utc>,
    #[serde(default)]
    pub content: String,
    #[serde(default)]
    pub has_link: bool,
    #[serde(default)]
    pub has_media: bool,
    #[serde(default)]
    pub has_file: bool,
    #[serde(default)]
    pub has_video: bool,
}

#[derive(Deserialize, Debug)]
pub struct SearchResponse {
    pub messages: Vec<Vec<Message>>,
    pub total_results: u64,
}
