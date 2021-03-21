use self::diesel::prelude::*;
use rocket::http::{ContentType, Status};
use rocket::request::Request;
use rocket::response;
use rocket::response::{Responder, Response};
use rocket_contrib::databases::diesel;
use rocket_contrib::json::{Json, JsonValue};

use super::database::AtuinDbConn;
use super::models::{NewHistory, User};
use crate::schema::history;

#[derive(Debug)]
pub struct ApiResponse {
    pub json: JsonValue,
    pub status: Status,
}

impl<'r> Responder<'r> for ApiResponse {
    fn respond_to(self, req: &Request) -> response::Result<'r> {
        Response::build_from(self.json.respond_to(req).unwrap())
            .status(self.status)
            .header(ContentType::JSON)
            .ok()
    }
}

#[get("/")]
pub const fn index() -> &'static str {
    "\"Through the fathomless deeps of space swims the star turtle Great A\u{2019}Tuin, bearing on its back the four giant elephants who carry on their shoulders the mass of the Discworld.\"\n\t-- Sir Terry Pratchett"
}

#[catch(500)]
pub fn internal_error(_req: &Request) -> ApiResponse {
    ApiResponse {
        status: Status::InternalServerError,
        json: json!({"status": "error", "message": "an internal server error has occured"}),
    }
}

#[catch(400)]
pub fn bad_request(_req: &Request) -> ApiResponse {
    ApiResponse {
        status: Status::InternalServerError,
        json: json!({"status": "error", "message": "bad request. don't do that."}),
    }
}

#[derive(Deserialize)]
pub struct AddHistory {
    id: String,
    timestamp: i64,
    data: String,
    mac: String,
}

#[post("/history", data = "<add_history>")]
#[allow(
    clippy::clippy::cast_sign_loss,
    clippy::cast_possible_truncation,
    clippy::clippy::needless_pass_by_value
)]
pub fn add_history(conn: AtuinDbConn, user: User, add_history: Json<AddHistory>) -> ApiResponse {
    let secs: i64 = add_history.timestamp / 1_000_000_000;
    let nanosecs: u32 = (add_history.timestamp - (secs * 1_000_000_000)) as u32;
    let datetime = chrono::NaiveDateTime::from_timestamp(secs, nanosecs);

    let new_history = NewHistory {
        client_id: add_history.id.as_str(),
        user_id: user.id,
        mac: add_history.mac.as_str(),
        timestamp: datetime,
        data: add_history.data.as_str(),
    };

    match diesel::insert_into(history::table)
        .values(&new_history)
        .execute(&*conn)
    {
        Ok(_) => ApiResponse {
            status: Status::Ok,
            json: json!({"status": "ok", "message": "history added", "id": new_history.client_id}),
        },
        Err(_) => ApiResponse {
            status: Status::BadRequest,
            json: json!({"status": "error", "message": "failed to add history"}),
        },
    }
}
