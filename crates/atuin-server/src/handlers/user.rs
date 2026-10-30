use std::borrow::Borrow;
use std::collections::HashMap;
use std::time::Duration;

use atuin_common::utils::crypto_random_string;
use atuin_domain::api::{
    ChangePasswordRequest, ChangePasswordResponse, DeleteUserRequest, DeleteUserResponse,
    LoginRequest, LoginResponse, RegisterRequest, RegisterResponse, UserResponse,
};
use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use metrics::counter;
use reqwest::header::CONTENT_TYPE;
use tracing::{debug, error, info, instrument, warn};

use super::{ErrorResponse, ErrorResponseStatus, RespExt};
use crate::client_ip::ClientIp;
use crate::db::DbError;
use crate::db::models::{HashedPassword, NewUser, NewUsername};
use crate::router::{AppState, UserAuth};

// Try to send a Discord webhook once - if it fails, we don't retry. "At most once", and best effort.
// Don't return the status because if this fails, we don't really care.
async fn send_register_hook(url: &url::Url, username: String, registered: String) {
    let hook = HashMap::from([
        ("username", username),
        ("content", format!("{registered} has just signed up!")),
    ]);

    let client = reqwest::Client::new();

    let resp = client
        .post(url.clone())
        .timeout(Duration::from_secs(5))
        .header(CONTENT_TYPE, "application/json")
        .json(&hook)
        .send()
        .await;

    match resp {
        Ok(_) => info!("register webhook sent ok!"),
        Err(e) => error!("failed to send register webhook: {}", e.without_url()),
    }
}

#[instrument(skip_all, err(level = "warn"), fields(user.username = username.as_str()))]
pub async fn get(
    Path(username): Path<String>,
    state: State<AppState>,
) -> Result<Json<UserResponse>, ErrorResponseStatus<'static>> {
    let db = &state.0.database;
    let user = match db.get_user(username.as_ref()).await {
        Ok(user) => user,
        Err(DbError::NotFound) => {
            debug!("user not found: {}", username);
            return Err(ErrorResponse::reply("user not found").with_status(StatusCode::NOT_FOUND));
        }
        Err(DbError::Other(err)) => {
            error!("database error: {}", err);
            return Err(ErrorResponse::reply("database error")
                .with_status(StatusCode::INTERNAL_SERVER_ERROR));
        }
    };

    Ok(Json(UserResponse {
        username: user.username,
    }))
}

#[instrument(skip_all, err(level = "warn"), fields(user.username = register.username.as_str()))]
pub async fn register(
    state: State<AppState>,
    Json(register): Json<RegisterRequest>,
) -> Result<Json<RegisterResponse>, ErrorResponseStatus<'static>> {
    if !state.settings.open_registration {
        return Err(ErrorResponse::reply("this server is not open for registrations")
            .with_status(StatusCode::BAD_REQUEST));
    }

    let Ok(username) = register.username.parse::<NewUsername>() else {
        return Err(ErrorResponse::reply(
            "Only alphanumeric and hyphens (-) are allowed in usernames",
        )
        .with_status(StatusCode::BAD_REQUEST));
    };

    let new_user = NewUser {
        email: register.email.clone(),
        username,
        password: HashedPassword::new(register.password).await,
    };

    // 24 bytes encoded as base64
    let token = crypto_random_string::<24>();

    // Create the user and their initial session atomically, so a failure can't leave an
    // account that exists but can never be logged into (or a dangling session).
    let db = &state.0.database;
    let user_id = match db.add_user_with_session(&new_user, &token).await {
        Ok(id) => id,
        Err(e) => {
            error!("failed to register user: {}", e);
            return Err(ErrorResponse::reply("failed to register user")
                .with_status(StatusCode::BAD_REQUEST));
        }
    };

    info!(user.id = user_id, "registered new user");

    if let Some(url) = &state.settings.register_webhook_url {
        // Could probs be run on another thread, but it's ok atm
        send_register_hook(
            url,
            state.settings.register_webhook_username.clone(),
            register.username,
        )
        .await;
    }

    counter!("atuin_users_registered").increment(1);

    Ok(Json(RegisterResponse {
        session: token,
        auth: Some("cli".into()),
    }))
}

#[instrument(skip_all, err(level = "warn"), fields(user.id = user.id, user.username = user.username.as_str()))]
pub async fn delete(
    UserAuth(user): UserAuth,
    state: State<AppState>,
    Json(delete): Json<DeleteUserRequest>,
) -> Result<Json<DeleteUserResponse>, ErrorResponseStatus<'static>> {
    debug!("request to delete user {}", user.id);

    if !user.password.verify(delete.password).await {
        return Err(
            ErrorResponse::reply("password is not correct").with_status(StatusCode::UNAUTHORIZED)
        );
    }

    let db = &state.0.database;
    if let Err(e) = db.delete_user(&user).await {
        error!("failed to delete user: {}", e);

        return Err(ErrorResponse::reply("failed to delete user")
            .with_status(StatusCode::INTERNAL_SERVER_ERROR));
    };

    counter!("atuin_users_deleted").increment(1);

    info!(user.id = user.id, "deleted user account");

    Ok(Json(DeleteUserResponse {}))
}

#[instrument(skip_all, err(level = "warn"), fields(user.id = user.id))]
pub async fn change_password(
    UserAuth(mut user): UserAuth,
    state: State<AppState>,
    Json(change_password): Json<ChangePasswordRequest>,
) -> Result<Json<ChangePasswordResponse>, ErrorResponseStatus<'static>> {
    let db = &state.0.database;

    if !user.password.verify(change_password.current_password).await {
        return Err(
            ErrorResponse::reply("password is not correct").with_status(StatusCode::UNAUTHORIZED)
        );
    }

    user.password = HashedPassword::new(change_password.new_password).await;

    if let Err(e) = db.update_user_password(&user).await {
        error!("failed to change user password: {}", e);

        return Err(ErrorResponse::reply("failed to change user password")
            .with_status(StatusCode::INTERNAL_SERVER_ERROR));
    };

    info!(user.id = user.id, "changed user password");

    Ok(Json(ChangePasswordResponse {}))
}

#[instrument(skip_all, err(level = "warn"), fields(client.ip = %ip, user.username = login.username.as_str()))]
pub async fn login(
    ClientIp(ip): ClientIp,
    state: State<AppState>,
    Json(login): Json<LoginRequest>,
) -> Result<Json<LoginResponse>, ErrorResponseStatus<'static>> {
    let db = &state.0.database;
    let user = match db.get_user(login.username.borrow()).await {
        Ok(u) => u,
        Err(DbError::NotFound) => {
            return Err(ErrorResponse::reply("user not found").with_status(StatusCode::NOT_FOUND));
        }
        Err(DbError::Other(e)) => {
            error!("failed to get user {}: {}", login.username.clone(), e);

            return Err(ErrorResponse::reply("database error")
                .with_status(StatusCode::INTERNAL_SERVER_ERROR));
        }
    };

    let session = match db.get_user_session(&user).await {
        Ok(u) => u,
        Err(DbError::NotFound) => {
            debug!("user session not found for user id={}", user.id);
            return Err(ErrorResponse::reply("user not found").with_status(StatusCode::NOT_FOUND));
        }
        Err(DbError::Other(err)) => {
            error!("database error for user {}: {}", login.username, err);
            return Err(ErrorResponse::reply("database error")
                .with_status(StatusCode::INTERNAL_SERVER_ERROR));
        }
    };

    if !user.password.verify(login.password).await {
        warn!(user.id = user.id, "login failed: incorrect password");
        return Err(
            ErrorResponse::reply("password is not correct").with_status(StatusCode::UNAUTHORIZED)
        );
    }

    info!(user.id = user.id, "login succeeded");

    Ok(Json(LoginResponse {
        session: session.token,
        auth: Some("cli".into()),
    }))
}
