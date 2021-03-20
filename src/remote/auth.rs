use self::diesel::prelude::*;
use rocket::http::Status;
use rocket::request::{self, FromRequest, Outcome, Request};
use rocket_contrib::databases::diesel;

use super::models::User;
use crate::schema::users;

use super::database::AtuinDbConn;

#[derive(Debug)]
pub enum KeyError {
    Missing,
    Invalid,
}

impl<'a, 'r> FromRequest<'a, 'r> for User {
    type Error = KeyError;

    fn from_request(request: &'a Request<'r>) -> request::Outcome<User, Self::Error> {
        let api: Vec<_> = request.headers().get("authorization").collect();

        if api.is_empty() {
            return Outcome::Failure((Status::BadRequest, KeyError::Missing));
        } else if api.len() > 1 {
            return Outcome::Failure((Status::BadRequest, KeyError::Invalid));
        }

        let api: Vec<_> = api[0].split(" ").collect();

        if api.len() != 2 {
            return Outcome::Failure((Status::BadRequest, KeyError::Invalid));
        }

        if api[0] != "Token" {
            return Outcome::Failure((Status::BadRequest, KeyError::Invalid));
        }

        let api = api[1];

        let db = request
            .guard::<AtuinDbConn>()
            .succeeded()
            .expect("failed to load database");

        let user = users::table.filter(users::api.eq(api)).first::<User>(&*db);

        match user {
            Ok(user) => Outcome::Success(user),
            Err(_) => Outcome::Failure((Status::BadRequest, KeyError::Invalid)),
        }
    }
}
