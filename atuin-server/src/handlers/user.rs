use std::borrow::Borrow;
use std::collections::HashMap;
use std::time::Duration;

use argon2::{
    password_hash::SaltString, Algorithm, Argon2, Params, PasswordHash, PasswordHasher,
    PasswordVerifier, Version,
};
use axum::{
    extract::{Path, State},
    Json,
};
use http::StatusCode;
use rand::rngs::OsRng;
use tracing::{debug, error, info, instrument};
use uuid::Uuid;

use super::{ErrorResponse, ErrorResponseStatus, RespExt};
use crate::{
    database::Database,
    models::{NewSession, NewUser},
    router::AppState,
};

use reqwest::header::CONTENT_TYPE;

use atuin_common::api::*;

pub fn verify_str(hash: &str, password: &str) -> bool {
    let arg2 = Argon2::new(Algorithm::Argon2id, Version::V0x13, Params::default());
    let Ok(hash) = PasswordHash::new(hash) else { return false };
    arg2.verify_password(password.as_bytes(), &hash).is_ok()
}

// Try to send a Discord webhook once - if it fails, we don't retry. "At most once", and best effort.
// Don't return the status because if this fails, we don't really care.
async fn send_register_hook(url: &str, username: String, registered: String) {
    let hook = HashMap::from([
        ("username", username),
        ("content", format!("{registered} has just signed up!")),
    ]);

    let client = reqwest::Client::new();

    let resp = client
        .post(url)
        .timeout(Duration::new(5, 0))
        .header(CONTENT_TYPE, "application/json")
        .json(&hook)
        .send()
        .await;

    match resp {
        Ok(_) => info!("register webhook sent ok!"),
        Err(e) => error!("failed to send register webhook: {}", e),
    }
}

#[instrument(skip_all, fields(user.username = username.as_str()))]
pub async fn get<DB: Database>(
    Path(username): Path<String>,
    state: State<AppState<DB>>,
) -> Result<Json<UserResponse>, ErrorResponseStatus<'static>> {
    let db = &state.0.database;
    let user = match db.get_user(username.as_ref()).await {
        Ok(user) => user,
        Err(sqlx::Error::RowNotFound) => {
            debug!("user not found: {}", username);
            return Err(ErrorResponse::reply("user not found").with_status(StatusCode::NOT_FOUND));
        }
        Err(err) => {
            error!("database error: {}", err);
            return Err(ErrorResponse::reply("database error")
                .with_status(StatusCode::INTERNAL_SERVER_ERROR));
        }
    };

    Ok(Json(UserResponse {
        username: user.username,
    }))
}

#[instrument(skip_all)]
pub async fn register<DB: Database>(
    state: State<AppState<DB>>,
    Json(register): Json<RegisterRequest>,
) -> Result<Json<RegisterResponse>, ErrorResponseStatus<'static>> {
    if !state.settings.open_registration {
        return Err(
            ErrorResponse::reply("this server is not open for registrations")
                .with_status(StatusCode::BAD_REQUEST),
        );
    }

    let hashed = hash_secret(&register.password);

    let new_user = NewUser {
        email: register.email.clone(),
        username: register.username.clone(),
        password: hashed,
    };

    let db = &state.0.database;
    let user_id = match db.add_user(&new_user).await {
        Ok(id) => id,
        Err(e) => {
            error!("failed to add user: {}", e);
            return Err(
                ErrorResponse::reply("failed to add user").with_status(StatusCode::BAD_REQUEST)
            );
        }
    };

    let token = Uuid::new_v4().as_simple().to_string();

    let new_session = NewSession {
        user_id,
        token: (&token).into(),
    };

    if let Some(url) = &state.settings.register_webhook_url {
        // Could probs be run on another thread, but it's ok atm
        send_register_hook(
            url,
            state.settings.register_webhook_username.clone(),
            register.username,
        )
        .await;
    }

    match db.add_session(&new_session).await {
        Ok(_) => Ok(Json(RegisterResponse { session: token })),
        Err(e) => {
            error!("failed to add session: {}", e);
            Err(ErrorResponse::reply("failed to register user")
                .with_status(StatusCode::BAD_REQUEST))
        }
    }
}

#[instrument(skip_all, fields(user.username = login.username.as_str()))]
pub async fn login<DB: Database>(
    state: State<AppState<DB>>,
    login: Json<LoginRequest>,
) -> Result<Json<LoginResponse>, ErrorResponseStatus<'static>> {
    let db = &state.0.database;
    let user = match db.get_user(login.username.borrow()).await {
        Ok(u) => u,
        Err(sqlx::Error::RowNotFound) => {
            return Err(ErrorResponse::reply("user not found").with_status(StatusCode::NOT_FOUND));
        }
        Err(e) => {
            error!("failed to get user {}: {}", login.username.clone(), e);

            return Err(ErrorResponse::reply("database error")
                .with_status(StatusCode::INTERNAL_SERVER_ERROR));
        }
    };

    let session = match db.get_user_session(&user).await {
        Ok(u) => u,
        Err(sqlx::Error::RowNotFound) => {
            debug!("user session not found for user id={}", user.id);
            return Err(ErrorResponse::reply("user not found").with_status(StatusCode::NOT_FOUND));
        }
        Err(err) => {
            error!("database error for user {}: {}", login.username, err);
            return Err(ErrorResponse::reply("database error")
                .with_status(StatusCode::INTERNAL_SERVER_ERROR));
        }
    };

    let verified = verify_str(user.password.as_str(), login.password.borrow());

    if !verified {
        return Err(ErrorResponse::reply("user not found").with_status(StatusCode::NOT_FOUND));
    }

    Ok(Json(LoginResponse {
        session: session.token,
    }))
}

fn hash_secret(password: &str) -> String {
    let arg2 = Argon2::new(Algorithm::Argon2id, Version::V0x13, Params::default());
    let salt = SaltString::generate(&mut OsRng);
    let hash = arg2.hash_password(password.as_bytes(), &salt).unwrap();
    hash.to_string()
}
