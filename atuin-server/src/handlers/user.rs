use std::borrow::Borrow;

use axum::{
    extract::{Path, State},
    Extension, Json,
};
use http::StatusCode;
use sodiumoxide::crypto::pwhash::argon2id13;
use tracing::{debug, error, instrument};
use uuid::Uuid;

use super::{ErrorResponse, ErrorResponseStatus, RespExt};
use crate::{
    database::Database,
    models::{NewSession, NewUser},
    router::AppState,
    settings::Settings,
};

use atuin_common::api::*;

pub fn verify_str(secret: &str, verify: &str) -> bool {
    sodiumoxide::init().unwrap();

    let mut padded = [0_u8; 128];
    secret.as_bytes().iter().enumerate().for_each(|(i, val)| {
        padded[i] = *val;
    });

    match argon2id13::HashedPassword::from_slice(&padded) {
        Some(hp) => argon2id13::pwhash_verify(&hp, verify.as_bytes()),
        None => false,
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
    settings: Extension<Settings>,
    state: State<AppState<DB>>,
    Json(register): Json<RegisterRequest>,
) -> Result<Json<RegisterResponse>, ErrorResponseStatus<'static>> {
    if !settings.open_registration {
        return Err(
            ErrorResponse::reply("this server is not open for registrations")
                .with_status(StatusCode::BAD_REQUEST),
        );
    }

    let hashed = hash_secret(&register.password);

    let new_user = NewUser {
        email: register.email,
        username: register.username,
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

fn hash_secret(secret: &str) -> String {
    sodiumoxide::init().unwrap();
    let hash = argon2id13::pwhash(
        secret.as_bytes(),
        argon2id13::OPSLIMIT_INTERACTIVE,
        argon2id13::MEMLIMIT_INTERACTIVE,
    )
    .unwrap();
    let texthash = std::str::from_utf8(&hash.0).unwrap().to_string();

    // postgres hates null chars. don't do that to postgres
    texthash.trim_end_matches('\u{0}').to_string()
}
