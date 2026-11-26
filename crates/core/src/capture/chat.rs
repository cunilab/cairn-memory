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

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "String")]
pub struct ChatPlatform(String);

impl TryFrom<String> for ChatPlatform {
    type Error = &'static str;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        let mut bytes = value.bytes();
        if value.len() <= 32
            && bytes.next().is_some_and(|b| b.is_ascii_lowercase())
            && bytes.all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_' || b == b'-')
        {
            Ok(Self(value))
        } else {
            Err("chat platform must be a lowercase token of at most 32 bytes")
        }
    }
}

/// Hashed transport context supplied by the operator's trusted gateway plugin.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChatContext {
    pub platform: ChatPlatform,
    pub kind: ChatKind,
    pub chat_id_hash: ChatHash,
    pub thread_id_hash: Option<ChatHash>,
}
