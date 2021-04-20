use std::convert::Infallible;

use sodiumoxide::crypto::pwhash::argon2id13;
use uuid::Uuid;
use warp::http::StatusCode;
use warp::reply::json;

use crate::api::{
    ErrorResponse, LoginRequest, LoginResponse, RegisterRequest, RegisterResponse, UserResponse,
};
use crate::server::database::Database;
use crate::server::models::{NewSession, NewUser};
use crate::settings::Settings;
use crate::utils::hash_secret;

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
) -> Result<Box<dyn warp::Reply>, Infallible> {
    let user = match db.get_user(username).await {
        Ok(user) => user,
        Err(e) => {
            debug!("user not found: {}", e);
            return Ok(Box::new(ErrorResponse::reply(
                "user not found",
                StatusCode::NOT_FOUND,
            )));
        }
    };

    Ok(Box::new(warp::reply::json(&UserResponse {
        username: user.username,
    })))
}

pub async fn register(
    register: RegisterRequest,
    settings: Settings,
    db: impl Database + Clone + Send + Sync,
) -> Result<Box<dyn warp::Reply>, Infallible> {
    if !settings.server.open_registration {
        return Ok(Box::new(ErrorResponse::reply(
            "this server is not open for registrations",
            StatusCode::BAD_REQUEST,
        )));
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
            return Ok(Box::new(ErrorResponse::reply(
                "failed to add user",
                StatusCode::BAD_REQUEST,
            )));
        }
    };

    let token = Uuid::new_v4().to_simple().to_string();

    let new_session = NewSession {
        user_id,
        token: token.as_str(),
    };

    match db.add_session(&new_session).await {
        Ok(_) => Ok(Box::new(json(&RegisterResponse { session: token }))),
        Err(e) => {
            error!("failed to add session: {}", e);
            Ok(Box::new(ErrorResponse::reply(
                "failed to register user",
                StatusCode::BAD_REQUEST,
            )))
        }
    }
}

pub async fn login(
    login: LoginRequest,
    db: impl Database + Clone + Send + Sync,
) -> Result<Box<dyn warp::Reply>, Infallible> {
    let user = match db.get_user(login.username.clone()).await {
        Ok(u) => u,
        Err(e) => {
            error!("failed to get user {}: {}", login.username.clone(), e);

            return Ok(Box::new(ErrorResponse::reply(
                "user not found",
                StatusCode::NOT_FOUND,
            )));
        }
    };

    let session = match db.get_user_session(&user).await {
        Ok(u) => u,
        Err(e) => {
            error!("failed to get session for {}: {}", login.username, e);

            return Ok(Box::new(ErrorResponse::reply(
                "user not found",
                StatusCode::NOT_FOUND,
            )));
        }
    };

    let verified = verify_str(user.password.as_str(), login.password.as_str());

    if !verified {
        return Ok(Box::new(ErrorResponse::reply(
            "user not found",
            StatusCode::NOT_FOUND,
        )));
    }

    Ok(Box::new(warp::reply::json(&LoginResponse {
        session: session.token,
    })))
}
