use self::diesel::prelude::*;
use eyre::Result;
use rocket::config::{Config, Environment, LoggingLevel, Value};
use rocket::http::{ContentType, Status};
use rocket::request::{self, Form, FromRequest, Outcome, Request};
use rocket::response;
use rocket::response::status::BadRequest;
use rocket::response::{Responder, Response};
use rocket_contrib::databases::diesel;
use rocket_contrib::json::{Json, JsonValue};

use std::collections::HashMap;
use uuid::Uuid;

use super::models::{NewUser, User};
use crate::remote::database::establish_connection;
use crate::schema::users;
use crate::settings::Settings;

use super::database::AtuinDbConn;

#[derive(Debug)]
pub struct ApiResponse {
    json: JsonValue,
    status: Status,
}

impl<'r> Responder<'r> for ApiResponse {
    fn respond_to(self, req: &Request) -> response::Result<'r> {
        Response::build_from(self.json.respond_to(&req).unwrap())
            .status(self.status)
            .header(ContentType::JSON)
            .ok()
    }
}

#[get("/")]
pub const fn index() -> &'static str {
    "\"Through the fathomless deeps of space swims the star turtle Great A’Tuin, bearing on its back the four giant elephants who carry on their shoulders the mass of the Discworld.\"\n\t-- Sir Terry Pratchett"
}

#[derive(FromForm)]
pub struct Register {
    email: String,
    key: String,
}

#[catch(500)]
pub fn internal_error<'a>(_req: &Request) -> ApiResponse {
    ApiResponse {
        status: Status::InternalServerError,
        json: json!({"status": "error", "message": "an internal server error has occured"}),
    }
}

#[catch(400)]
pub fn bad_request<'a>(_req: &Request) -> ApiResponse {
    ApiResponse {
        status: Status::InternalServerError,
        json: json!({"status": "error", "message": "bad request. don't do that."}),
    }
}

#[post("/register", data = "<register>")]
pub fn register<'a>(conn: AtuinDbConn, register: Form<Register>) -> ApiResponse {
    // TODO: allow rolling this
    let api_key = Uuid::new_v4().to_simple().to_string();

    let new_user = NewUser {
        email: register.email.as_str(),
        key: register.key.as_str(),
        api: api_key.as_str(),
    };

    match diesel::insert_into(users::table)
        .values(&new_user)
        .execute(&*conn)
    {
        Ok(_) => ApiResponse {
            status: Status::Ok,
            json: json!({"status": "ok", "message": "user created!", "api_key": api_key}),
        },
        Err(_) => ApiResponse {
            status: Status::BadRequest,
            json: json!({"status": "error", "message": "failed to create user"}),
        },
    }
}

#[post("/history")]
pub fn add_history<'a>(user: User) -> ApiResponse {
    add in the history here
    sort encryption and stuff too!
    ApiResponse {
        status: Status::Ok,
        json: json!({"status": "ok", "message": user.email}),
    }
}
