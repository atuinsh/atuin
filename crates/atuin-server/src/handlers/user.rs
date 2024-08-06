use std::borrow::Borrow;
use std::collections::HashMap;
use std::time::Duration;

use argon2::{
    password_hash::SaltString, Algorithm, Argon2, Params, PasswordHash, PasswordHasher,
    PasswordVerifier, Version,
};
use axum::{
    extract::{Path, State},
    http::StatusCode,
    Json,
};
use metrics::counter;

use postmark::{reqwest::PostmarkClient, Query};

use rand::rngs::OsRng;
use tracing::{debug, error, info, instrument};

use super::{ErrorResponse, ErrorResponseStatus, RespExt};
use crate::router::{AppState, UserAuth};
use atuin_server_database::{
    models::{NewSession, NewUser},
    Database, DbError,
};

use reqwest::header::CONTENT_TYPE;

use atuin_common::{api::*, utils::crypto_random_string};

pub fn verify_str(hash: &str, password: &str) -> bool {
    let arg2 = Argon2::new(Algorithm::Argon2id, Version::V0x13, Params::default());
    let Ok(hash) = PasswordHash::new(hash) else {
        return false;
    };
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

    for c in register.username.chars() {
        match c {
            'a'..='z' | 'A'..='Z' | '0'..='9' | '-' => {}
            _ => {
                return Err(ErrorResponse::reply(
                    "Only alphanumeric and hyphens (-) are allowed in usernames",
                )
                .with_status(StatusCode::BAD_REQUEST))
            }
        }
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

    // 24 bytes encoded as base64
    let token = crypto_random_string::<24>();

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

    counter!("atuin_users_registered", 1);

    match db.add_session(&new_session).await {
        Ok(_) => Ok(Json(RegisterResponse { session: token })),
        Err(e) => {
            error!("failed to add session: {}", e);
            Err(ErrorResponse::reply("failed to register user")
                .with_status(StatusCode::BAD_REQUEST))
        }
    }
}

#[instrument(skip_all, fields(user.id = user.id))]
pub async fn delete<DB: Database>(
    UserAuth(user): UserAuth,
    state: State<AppState<DB>>,
) -> Result<Json<DeleteUserResponse>, ErrorResponseStatus<'static>> {
    debug!("request to delete user {}", user.id);

    let db = &state.0.database;
    if let Err(e) = db.delete_user(&user).await {
        error!("failed to delete user: {}", e);

        return Err(ErrorResponse::reply("failed to delete user")
            .with_status(StatusCode::INTERNAL_SERVER_ERROR));
    };

    counter!("atuin_users_deleted", 1);

    Ok(Json(DeleteUserResponse {}))
}

#[instrument(skip_all, fields(user.id = user.id))]
pub async fn send_verification<DB: Database>(
    UserAuth(user): UserAuth,
    state: State<AppState<DB>>,
) -> Result<Json<SendVerificationResponse>, ErrorResponseStatus<'static>> {
    let settings = state.0.settings;
    debug!("request to verify user {}", user.username);

    if !settings.mail.enabled {
        return Ok(Json(SendVerificationResponse {
            email_sent: false,
            verified: false,
        }));
    }

    if user.verified.is_some() {
        return Ok(Json(SendVerificationResponse {
            email_sent: false,
            verified: true,
        }));
    }

    // TODO: if we ever add another mail provider, can match on them all here.
    let postmark_token = if let Some(token) = settings.mail.postmark.token {
        token
    } else {
        error!("Failed to verify email: got None for postmark token");
        return Err(ErrorResponse::reply("mail not configured")
            .with_status(StatusCode::INTERNAL_SERVER_ERROR));
    };

    let db = &state.0.database;

    let verification_token = db
        .user_verification_token(user.id)
        .await
        .expect("Failed to verify");

    debug!("Generated verification token, emailing user");

    let client = PostmarkClient::builder()
        .base_url("https://api.postmarkapp.com/")
        .token(postmark_token)
        .build();

    let req = postmark::api::email::SendEmailRequest::builder()
        .from(settings.mail.verification.from)
        .subject(settings.mail.verification.subject)
        .to(user.email)
        .body(postmark::api::Body::text(format!(
            "Please run the following command to finalize your Atuin account verification. It is valid for 15 minutes:\n\natuin account verify --token '{}'",
            verification_token
        )))
        .build();

    req.execute(&client)
        .await
        .expect("postmark email request failed");

    debug!("Email sent");

    Ok(Json(SendVerificationResponse {
        email_sent: true,
        verified: false,
    }))
}

#[instrument(skip_all, fields(user.id = user.id))]
pub async fn verify_user<DB: Database>(
    UserAuth(user): UserAuth,
    state: State<AppState<DB>>,
    Json(token_request): Json<VerificationTokenRequest>,
) -> Result<Json<VerificationTokenResponse>, ErrorResponseStatus<'static>> {
    let db = state.0.database;

    if user.verified.is_some() {
        return Ok(Json(VerificationTokenResponse { verified: true }));
    }

    let token = db.user_verification_token(user.id).await.map_err(|e| {
        error!("Failed to read user token: {e}");

        ErrorResponse::reply("Failed to verify").with_status(StatusCode::INTERNAL_SERVER_ERROR)
    })?;

    if token_request.token == token {
        db.verify_user(user.id).await.map_err(|e| {
            error!("Failed to verify user: {e}");

            ErrorResponse::reply("Failed to verify").with_status(StatusCode::INTERNAL_SERVER_ERROR)
        })?;
    } else {
        info!(
            "Incorrect verification token {} vs {}",
            token_request.token, token
        );

        return Ok(Json(VerificationTokenResponse { verified: false }));
    }

    Ok(Json(VerificationTokenResponse { verified: true }))
}

#[instrument(skip_all, fields(user.id = user.id, change_password))]
pub async fn change_password<DB: Database>(
    UserAuth(mut user): UserAuth,
    state: State<AppState<DB>>,
    Json(change_password): Json<ChangePasswordRequest>,
) -> Result<Json<ChangePasswordResponse>, ErrorResponseStatus<'static>> {
    let db = &state.0.database;

    let verified = verify_str(
        user.password.as_str(),
        change_password.current_password.borrow(),
    );
    if !verified {
        return Err(
            ErrorResponse::reply("password is not correct").with_status(StatusCode::UNAUTHORIZED)
        );
    }

    let hashed = hash_secret(&change_password.new_password);
    user.password = hashed;

    if let Err(e) = db.update_user_password(&user).await {
        error!("failed to change user password: {}", e);

        return Err(ErrorResponse::reply("failed to change user password")
            .with_status(StatusCode::INTERNAL_SERVER_ERROR));
    };
    Ok(Json(ChangePasswordResponse {}))
}

#[instrument(skip_all, fields(user.username = login.username.as_str()))]
pub async fn login<DB: Database>(
    state: State<AppState<DB>>,
    login: Json<LoginRequest>,
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

    let verified = verify_str(user.password.as_str(), login.password.borrow());

    if !verified {
        debug!(user = user.username, "login failed");
        return Err(
            ErrorResponse::reply("password is not correct").with_status(StatusCode::UNAUTHORIZED)
        );
    }

    debug!(user = user.username, "login success");

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
