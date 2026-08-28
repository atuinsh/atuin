//! Plain data-transfer types returned by / passed to the [`Database`](super::Database)
//! trait. Row decoding now lives in the sea-orm [`entities`](super::entities); these are
//! the backend-agnostic shapes the handlers see, mapped from entity `Model`s in
//! [`super`].

pub struct User {
    pub id: i64,
    pub username: String,
    pub email: String,
    pub password: String,
}

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
