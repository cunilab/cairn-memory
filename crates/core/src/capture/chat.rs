use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "String")]
pub struct ChatHash(String);

impl ChatHash {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for ChatHash {
    type Error = &'static str;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        if value.len() == 64
            && value
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            Ok(Self(value))
        } else {
            Err("chat identifiers must be lowercase SHA-256 digests")
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChatKind {
    Dm,
    Group,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChatPlatform {
    Telegram,
    Discord,
}

/// Hashed transport context supplied by the operator's trusted gateway plugin.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChatContext {
    pub platform: ChatPlatform,
    pub kind: ChatKind,
    pub chat_id_hash: ChatHash,
    pub sender_id_hash: Option<ChatHash>,
    pub thread_id_hash: Option<ChatHash>,
    #[serde(default)]
    pub owner: bool,
}

impl ChatContext {
    pub fn owner_dm(&self) -> bool {
        self.kind == ChatKind::Dm && self.owner && self.sender_id_hash.is_some()
    }
}
