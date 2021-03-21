use self::diesel::prelude::*;
use rocket::http::Status;
use rocket::request::{self, FromRequest, Outcome, Request};
use rocket_contrib::databases::diesel;
use sodiumoxide::crypto::pwhash::argon2id13;

use rocket_contrib::json::Json;
use uuid::Uuid;

use super::models::{NewSession, NewUser, Session, User};
use super::views::ApiResponse;
use crate::schema::{sessions, users};

use super::database::AtuinDbConn;

#[derive(Debug)]
pub enum KeyError {
    Missing,
    Invalid,
}

pub fn hash_str(secret: &str) -> String {
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

impl<'a, 'r> FromRequest<'a, 'r> for User {
    type Error = KeyError;

    fn from_request(request: &'a Request<'r>) -> request::Outcome<User, Self::Error> {
        let session: Vec<_> = request.headers().get("authorization").collect();

        if session.is_empty() {
            return Outcome::Failure((Status::BadRequest, KeyError::Missing));
        } else if session.len() > 1 {
            return Outcome::Failure((Status::BadRequest, KeyError::Invalid));
        }

        let session: Vec<_> = session[0].split(' ').collect();

        if session.len() != 2 {
            return Outcome::Failure((Status::BadRequest, KeyError::Invalid));
        }

        if session[0] != "Token" {
            return Outcome::Failure((Status::BadRequest, KeyError::Invalid));
        }

        let session = session[1];

        let db = request
            .guard::<AtuinDbConn>()
            .succeeded()
            .expect("failed to load database");

        let session = sessions::table
            .filter(sessions::token.eq(session))
            .first::<Session>(&*db);

        if session.is_err() {
            return Outcome::Failure((Status::Unauthorized, KeyError::Invalid));
        }

        let session = session.unwrap();

        let user = users::table.find(session.user_id).first(&*db);

        match user {
            Ok(user) => Outcome::Success(user),
            Err(_) => Outcome::Failure((Status::Unauthorized, KeyError::Invalid)),
        }
    }
}

#[derive(Deserialize)]
pub struct Register {
    email: String,
    password: String,
}

#[post("/register", data = "<register>")]
#[allow(clippy::clippy::needless_pass_by_value)]
pub fn register(conn: AtuinDbConn, register: Json<Register>) -> ApiResponse {
    let hashed = hash_str(register.password.as_str());

    let new_user = NewUser {
        email: register.email.as_str(),
        password: hashed.as_str(),
    };

    let user = diesel::insert_into(users::table)
        .values(&new_user)
        .get_result(&*conn);

    if user.is_err() {
        return ApiResponse {
            status: Status::BadRequest,
            json: json!({
                "status": "error",
                "message": "failed to create user - is the email already in use?",
            }),
        };
    }

    let user: User = user.unwrap();
    let token = Uuid::new_v4().to_simple().to_string();

    let new_session = NewSession {
        user_id: user.id,
        token: token.as_str(),
    };

    match diesel::insert_into(sessions::table)
        .values(&new_session)
        .execute(&*conn)
    {
        Ok(_) => ApiResponse {
            status: Status::Ok,
            json: json!({"status": "ok", "message": "user created!", "session": token}),
        },
        Err(_) => ApiResponse {
            status: Status::BadRequest,
            json: json!({"status": "error", "message": "failed to create user"}),
        },
    }
}

#[derive(Deserialize)]
pub struct Login {
    email: String,
    password: String,
}

#[post("/login", data = "<login>")]
#[allow(clippy::clippy::needless_pass_by_value)]
pub fn login(conn: AtuinDbConn, login: Json<Login>) -> ApiResponse {
    let user = users::table
        .filter(users::email.eq(login.email.as_str()))
        .first(&*conn);

    if user.is_err() {
        return ApiResponse {
            status: Status::NotFound,
            json: json!({"status": "error", "message": "user not found"}),
        };
    }

    let user: User = user.unwrap();

    let session = sessions::table
        .filter(sessions::user_id.eq(user.id))
        .first(&*conn);

    // a session should exist...
    if session.is_err() {
        return ApiResponse {
            status: Status::InternalServerError,
            json: json!({"status": "error", "message": "something went wrong"}),
        };
    }

    let verified = verify_str(user.password.as_str(), login.password.as_str());

    if !verified {
        return ApiResponse {
            status: Status::NotFound,
            json: json!({"status": "error", "message": "user not found"}),
        };
    }

    let session: Session = session.unwrap();

    ApiResponse {
        status: Status::Ok,
        json: json!({"status": "ok", "token": session.token}),
    }
}
