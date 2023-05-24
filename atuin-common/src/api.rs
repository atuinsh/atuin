use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::borrow::Cow;

#[derive(Debug, Serialize, Deserialize)]
pub struct UserResponse {
    pub username: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct RegisterRequest {
    pub email: String,
    pub username: String,
    pub password: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct RegisterResponse {
    pub session: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct DeleteUserResponse {}

#[derive(Debug, Serialize, Deserialize)]
pub struct LoginRequest {
    pub username: String,
    pub password: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct LoginResponse {
    pub session: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct AddHistoryRequest {
    pub id: String,
    pub timestamp: chrono::DateTime<Utc>,
    pub data: String,
    pub hostname: String,
    pub encryption: Option<EncryptionScheme>,
}

#[derive(Debug, Clone)]
pub enum EncryptionScheme {
    /// Encryption scheme using xsalsa20poly1305 (tweetnacl crypto_box) using the legacy system
    /// with no additional data and the same key for each entry with random IV
    XSalsa20Poly1305Legacy,

    /// Encryption scheme using xchacha20poly1305. Entry id is used in the additional data.
    /// The key is derived from the original using the ID as info and "history" as the salt.
    /// Each entry uses a random IV too.
    XChaCha20Poly1305,

    Unknown(String),
}

impl EncryptionScheme {
    pub fn to_str(&self) -> &str {
        match self {
            EncryptionScheme::XSalsa20Poly1305Legacy => "XSalsa20Poly1305Legacy",
            EncryptionScheme::XChaCha20Poly1305 => "XChaCha20Poly1305",
            EncryptionScheme::Unknown(x) => x,
        }
    }
    pub fn from_string(s: String) -> Self {
        match &*s {
            "XSalsa20Poly1305Legacy" => EncryptionScheme::XSalsa20Poly1305Legacy,
            "XChaCha20Poly1305" => EncryptionScheme::XChaCha20Poly1305,
            _ => EncryptionScheme::Unknown(s),
        }
    }
}

impl Serialize for EncryptionScheme {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        self.to_str().serialize(serializer)
    }
}
impl<'de> Deserialize<'de> for EncryptionScheme {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        Ok(Self::from_string(s))
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct CountResponse {
    pub count: i64,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct SyncHistoryRequest {
    pub sync_ts: chrono::DateTime<chrono::FixedOffset>,
    pub history_ts: chrono::DateTime<chrono::FixedOffset>,
    pub host: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct SyncHistoryResponse {
    /// deprecated
    pub history: Vec<String>,
    pub sync_history: Vec<SyncHistoryItem>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct SyncHistoryItem {
    pub id: String,
    pub data: String,
    pub encryption: Option<EncryptionScheme>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ErrorResponse<'a> {
    pub reason: Cow<'a, str>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct IndexResponse {
    pub homage: String,
    pub version: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct StatusResponse {
    pub count: i64,
    pub username: String,
    pub deleted: Vec<String>,

    // These could/should also go on the index of the server
    // However, we do not request the server index as a part of normal sync
    // I'd rather slightly increase the size of this response, than add an extra HTTP request
    pub page_size: i64, // max page size supported by the server
    pub version: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct DeleteHistoryRequest {
    pub client_id: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct MessageResponse {
    pub message: String,
}
