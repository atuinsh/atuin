use atuin_common::api::*;
use atuin_common::utils::hash_secret;
use sodiumoxide::crypto::pwhash::argon2id13;
use uuid::Uuid;
use warp::http::StatusCode;

use crate::database::Database;
use crate::models::{NewSession, NewUser};
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
    username: String,
    db: impl Database + Clone + Send + Sync,
) -> JSONResponse<UserResponse> {
    let user = match db.get_user(username).await {
        Ok(user) => user,
        Err(e) => {
            debug!("user not found: {}", e);
            return json_error(
                ErrorResponse::reply("user not found").with_status(StatusCode::NOT_FOUND),
            );
        }
    };

    json(UserResponse {
        username: user.username,
    })
}

pub async fn register(
    register: RegisterRequest,
    settings: Settings,
    db: impl Database + Clone + Send + Sync,
) -> JSONResponse<RegisterResponse> {
    if !settings.open_registration {
        return json_error(
            ErrorResponse::reply("this server is not open for registrations")
                .with_status(StatusCode::BAD_REQUEST),
        );
    }

    let hashed = hash_secret(register.password.as_str());

    let new_user = NewUser {
        email: register.email,
        username: register.username,
        password: hashed,
    };

    let user_id = match db.add_user(new_user).await {
        Ok(id) => id,
        Err(e) => {
            error!("failed to add user: {}", e);
            return json_error(
                ErrorResponse::reply("failed to add user").with_status(StatusCode::BAD_REQUEST),
            );
        }
    };

    let token = Uuid::new_v4().to_simple().to_string();

    let new_session = NewSession {
        user_id,
        token: token.as_str(),
    };

    match db.add_session(&new_session).await {
        Ok(_) => json(RegisterResponse { session: token }),
        Err(e) => {
            error!("failed to add session: {}", e);
            json_error(
                ErrorResponse::reply("failed to register user")
                    .with_status(StatusCode::BAD_REQUEST),
            )
        }
    }
}

pub async fn login(
    login: LoginRequest,
    db: impl Database + Clone + Send + Sync,
) -> JSONResponse<LoginResponse> {
    let user = match db.get_user(login.username.clone()).await {
        Ok(u) => u,
        Err(e) => {
            error!("failed to get user {}: {}", login.username.clone(), e);

            return json_error(
                ErrorResponse::reply("user not found").with_status(StatusCode::NOT_FOUND),
            );
        }
    };

    let session = match db.get_user_session(&user).await {
        Ok(u) => u,
        Err(e) => {
            error!("failed to get session for {}: {}", login.username, e);

            return json_error(
                ErrorResponse::reply("user not found").with_status(StatusCode::NOT_FOUND),
            );
        }
    };

    let verified = verify_str(user.password.as_str(), login.password.as_str());

    if !verified {
        return json_error(
            ErrorResponse::reply("user not found").with_status(StatusCode::NOT_FOUND),
        );
    }

    json(LoginResponse {
        session: session.token,
    })
}
