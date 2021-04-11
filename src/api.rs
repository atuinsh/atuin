use chrono::prelude::*;
use chrono::Utc;

use crate::local::encryption::EncryptedHistory;
use crate::remote::models::History;

// This is shared between the client and the server, and has the data structures
// representing the requests/responses for each method.
// TODO: Properly define responses rather than using json!

#[derive(Debug, Serialize, Deserialize)]
pub struct RegisterRequest {
    pub email: String,
    pub username: String,
    pub password: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct LoginRequest {
    pub username: String,
    pub password: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct AddHistoryRequest {
    pub id: String,
    pub timestamp: chrono::DateTime<Utc>,
    pub data: String,
    pub hostname: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct CountResponse {
    pub count: i64,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ListHistoryResponse {
    pub history: Vec<String>,
}
