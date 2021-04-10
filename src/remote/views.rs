use chrono::Utc;
use rocket::http::uri::Uri;
use rocket::http::RawStr;
use rocket::http::{ContentType, Status};
use rocket::request::FromFormValue;
use rocket::request::Request;
use rocket::response;
use rocket::response::{Responder, Response};
use rocket_contrib::databases::diesel;
use rocket_contrib::json::{Json, JsonValue};

use self::diesel::prelude::*;

use crate::api::AddHistoryRequest;
use crate::schema::history;

use super::database::AtuinDbConn;
use super::models::{History, NewHistory, User};

fn nsec_to_ts(nanos: i64) -> chrono::NaiveDateTime {
    let secs: i64 = nanos / 1_000_000_000;
    let nanosecs: u32 = (nanos - (secs * 1_000_000_000)) as u32;
    let datetime = chrono::NaiveDateTime::from_timestamp(secs, nanosecs);

    datetime
}

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

#[post("/history", data = "<add_history>")]
#[allow(
    clippy::clippy::cast_sign_loss,
    clippy::cast_possible_truncation,
    clippy::clippy::needless_pass_by_value
)]
pub fn add_history(
    conn: AtuinDbConn,
    user: User,
    add_history: Json<Vec<AddHistoryRequest>>,
) -> ApiResponse {
    let new_history: Vec<NewHistory> = add_history
        .iter()
        .map(|h| NewHistory {
            client_id: h.id.as_str(),
            user_id: user.id,
            timestamp: h.timestamp.naive_utc(),
            data: h.data.as_str(),
        })
        .collect();

    match diesel::insert_into(history::table)
        .values(&new_history)
        .on_conflict_do_nothing()
        .execute(&*conn)
    {
        Ok(_) => ApiResponse {
            status: Status::Ok,
            json: json!({"status": "ok", "message": "history added"}),
        },
        Err(_) => ApiResponse {
            status: Status::BadRequest,
            json: json!({"status": "error", "message": "failed to add history"}),
        },
    }
}

#[get("/sync/count")]
#[allow(clippy::wildcard_imports, clippy::needless_pass_by_value)]
pub fn sync_count(conn: AtuinDbConn, user: User) -> ApiResponse {
    use crate::schema::history::dsl::*;

    // we need to return the number of history items we have for this user
    // in the future I'd like to use something like a merkel tree to calculate
    // which day specifically needs syncing
    let count = history
        .filter(user_id.eq(user.id))
        .count()
        .first::<i64>(&*conn);

    if count.is_err() {
        error!("failed to count: {}", count.err().unwrap());

        return ApiResponse {
            json: json!({"message": "internal server error"}),
            status: Status::InternalServerError,
        };
    }

    ApiResponse {
        status: Status::Ok,
        json: json!({"count": count.ok()}),
    }
}

pub struct UtcDateTime(chrono::DateTime<Utc>);

impl<'v> FromFormValue<'v> for UtcDateTime {
    type Error = &'v RawStr;

    fn from_form_value(form_value: &'v RawStr) -> Result<UtcDateTime, &'v RawStr> {
        let time = Uri::percent_decode(form_value.as_bytes()).map_err(|_| form_value)?;
        let time = time.to_string();
        info!("got time {}", time);

        match chrono::DateTime::parse_from_rfc3339(time.as_str()) {
            Ok(t) => Ok(UtcDateTime(t.with_timezone(&Utc))),
            Err(e) => {
                error!("failed to parse time {}, got: {}", time, e);
                Err(form_value)
            }
        }
    }
}

// Request a list of all history items since a given timestamp. The timestamp is
// expected to be given as nanoseconds since 1st Jan 1970, UTC.
#[get("/history?<before>")]
#[allow(clippy::wildcard_imports, clippy::needless_pass_by_value)]
pub fn sync_list(conn: AtuinDbConn, user: User, before: UtcDateTime) -> ApiResponse {
    use crate::schema::history::dsl::*;

    // we need to return the number of history items we have for this user
    // in the future I'd like to use something like a merkel tree to calculate
    // which day specifically needs syncing
    // TODO: Allow for configuring the page size, both from params, and setting
    // the max in config. 100 is fine for now.
    let h = history
        .filter(user_id.eq(user.id))
        .filter(timestamp.le(before.0.naive_utc()))
        .order(timestamp.desc())
        .limit(100)
        .load::<History>(&*conn);

    if h.is_err() {
        error!("failed to list: {}", h.err().unwrap());

        return ApiResponse {
            json: json!({"message": "internal server error"}),
            status: Status::InternalServerError,
        };
    }

    let user_data: Vec<String> = h.unwrap().iter().map(|i| i.data.to_string()).collect();

    ApiResponse {
        status: Status::Ok,
        json: json!({ "history": user_data }),
    }
}
