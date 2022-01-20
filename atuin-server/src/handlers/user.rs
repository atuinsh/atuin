use std::borrow::{Borrow, Cow};

use atuin_common::api::*;
use atuin_common::utils::hash_secret;
use sodiumoxide::crypto::pwhash::argon2id13;
use uuid::Uuid;
use warp::http::StatusCode;

use crate::database::Database;
use crate::models::{NewSession, NewUser, User};
use crate::settings::Settings;

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

pub async fn get(
    username: impl AsRef<str>,
    db: impl Database + Clone + Send + Sync,
) -> JSONResult<ErrorResponseStatus<'static>> {
    let user = match db.get_user(username.as_ref()).await {
        Ok(user) => user,
        Err(e) => {
            debug!("user not found: {}", e);
            return reply_error(
                ErrorResponse::reply("user not found").with_status(StatusCode::NOT_FOUND),
            );
        }
    };

    reply_json(UserResponse {
        username: user.username.into(),
    })
}

pub async fn delete(
    user: User,
    db: impl Database + Clone + Send + Sync,
) -> JSONResult<ErrorResponseStatus<'static>> {
    let _ = match db.purge_history(&user).await {
        Ok(c) => c,
        Err(e) => {
            error!("failed to purge history: {}", e);

            return reply_error(
                ErrorResponse::reply("failed to purge history")
                    .with_status(StatusCode::INTERNAL_SERVER_ERROR),
            );
        }
    };

    let _ = match db.delete_user_sessions(&user).await {
        Ok(c) => c,
        Err(e) => {
            error!("failed to delete sessions: {}", e);

            return reply_error(
                ErrorResponse::reply("failed to delete sessions")
                    .with_status(StatusCode::INTERNAL_SERVER_ERROR),
            );
        }
    };

    let _ = match db.delete_user(user.username.as_str()).await {
        Ok(c) => c,
        Err(e) => {
            error!("failed to delete user: {}", e);

            return reply_error(
                ErrorResponse::reply("failed to delete user")
                    .with_status(StatusCode::INTERNAL_SERVER_ERROR),
            );
        }
    };

    reply_json(DeleteResponse {
        message: Cow::from("account deleted"),
    })
}

pub async fn register(
    register: RegisterRequest<'_>,
    settings: Settings,
    db: impl Database + Clone + Send + Sync,
) -> JSONResult<ErrorResponseStatus<'static>> {
    if !settings.open_registration {
        return reply_error(
            ErrorResponse::reply("this server is not open for registrations")
                .with_status(StatusCode::BAD_REQUEST),
        );
    }

    let hashed = hash_secret(&register.password);

    let new_user = NewUser {
        email: register.email,
        username: register.username,
        password: hashed.into(),
    };

    let user_id = match db.add_user(&new_user).await {
        Ok(id) => id,
        Err(e) => {
            error!("failed to add user: {}", e);
            return reply_error(
                ErrorResponse::reply("failed to add user").with_status(StatusCode::BAD_REQUEST),
            );
        }
    };

    let token = Uuid::new_v4().to_simple().to_string();

    let new_session = NewSession {
        user_id,
        token: (&token).into(),
    };

    match db.add_session(&new_session).await {
        Ok(_) => reply_json(RegisterResponse {
            session: token.into(),
        }),
        Err(e) => {
            error!("failed to add session: {}", e);
            reply_error(
                ErrorResponse::reply("failed to register user")
                    .with_status(StatusCode::BAD_REQUEST),
            )
        }
    }
}

pub async fn login(
    login: LoginRequest<'_>,
    db: impl Database + Clone + Send + Sync,
) -> JSONResult<ErrorResponseStatus<'_>> {
    let user = match db.get_user(login.username.borrow()).await {
        Ok(u) => u,
        Err(e) => {
            error!("failed to get user {}: {}", login.username.clone(), e);

            return reply_error(
                ErrorResponse::reply("user not found").with_status(StatusCode::NOT_FOUND),
            );
        }
    };

    let session = match db.get_user_session(&user).await {
        Ok(u) => u,
        Err(e) => {
            error!("failed to get session for {}: {}", login.username, e);

            return reply_error(
                ErrorResponse::reply("user not found").with_status(StatusCode::NOT_FOUND),
            );
        }
    };

    let verified = verify_str(user.password.as_str(), login.password.borrow());

    if !verified {
        return reply_error(
            ErrorResponse::reply("user not found").with_status(StatusCode::NOT_FOUND),
        );
    }

    reply_json(LoginResponse {
        session: session.token.into(),
    })
}
