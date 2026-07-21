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
