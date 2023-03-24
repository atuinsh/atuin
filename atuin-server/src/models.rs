use sqlx::types::time::OffsetDateTime;

#[derive(sqlx::FromRow)]
pub struct History {
    pub id: i64,
    pub client_id: String, // a client generated ID
    pub user_id: i64,
    pub hostname: String,
    pub timestamp: OffsetDateTime,

    pub data: String,

    pub created_at: OffsetDateTime,
}

pub struct NewHistory {
    pub client_id: String,
    pub user_id: i64,
    pub hostname: String,
    pub timestamp: OffsetDateTime,

    pub data: String,
}

#[derive(sqlx::FromRow)]
pub struct User {
    pub id: i64,
    pub username: String,
    pub email: String,
    pub password: String,
}

#[derive(sqlx::FromRow)]
pub struct Session {
    pub id: i64,
    pub user_id: i64,
    pub token: String,
}

pub struct NewUser {
    pub username: String,
    pub email: String,
    pub password: String,
}

pub struct NewSession {
    pub user_id: i64,
    pub token: String,
}
