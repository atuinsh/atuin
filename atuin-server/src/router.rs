use std::convert::Infallible;

use eyre::Result;
use warp::{hyper::StatusCode, Filter};

use atuin_common::api::SyncHistoryRequest;

use super::handlers;
use super::{database::Database, database::Postgres};
use crate::models::User;
use crate::settings::Settings;

fn with_settings(
    settings: Settings,
) -> impl Filter<Extract = (Settings,), Error = Infallible> + Clone {
    warp::any().map(move || settings.clone())
}

fn with_db(
    db: impl Database + Clone + Send + Sync,
) -> impl Filter<Extract = (impl Database + Clone,), Error = Infallible> + Clone {
    warp::any().map(move || db.clone())
}

fn with_user(
    postgres: Postgres,
) -> impl Filter<Extract = (User,), Error = warp::Rejection> + Clone {
    warp::header::<String>("authorization").and_then(move |header: String| {
        // async closures are still buggy :(
        let postgres = postgres.clone();

        async move {
            let header: Vec<&str> = header.split(' ').collect();

            let token;

            if header.len() == 2 {
                if header[0] != "Token" {
                    return Err(warp::reject());
                }

                token = header[1];
            } else {
                return Err(warp::reject());
            }

            let user = postgres
                .get_session_user(token)
                .await
                .map_err(|_| warp::reject())?;

            Ok(user)
        }
    })
}

pub async fn router(
    settings: &Settings,
) -> Result<impl Filter<Extract = impl warp::Reply, Error = Infallible> + Clone> {
    let postgres = Postgres::new(settings.db_uri.as_str()).await?;
    let index = warp::get().and(warp::path::end()).map(handlers::index);

    let count = warp::get()
        .and(warp::path("sync"))
        .and(warp::path("count"))
        .and(warp::path::end())
        .and(with_user(postgres.clone()))
        .and(with_db(postgres.clone()))
        .and_then(handlers::history::count);

    let sync = warp::get()
        .and(warp::path("sync"))
        .and(warp::path("history"))
        .and(warp::query::<SyncHistoryRequest>())
        .and(warp::path::end())
        .and(with_user(postgres.clone()))
        .and(with_db(postgres.clone()))
        .and_then(handlers::history::list);

    let add_history = warp::post()
        .and(warp::path("history"))
        .and(warp::path::end())
        .and(warp::body::json())
        .and(with_user(postgres.clone()))
        .and(with_db(postgres.clone()))
        .and_then(handlers::history::add);

    let user = warp::get()
        .and(warp::path("user"))
        .and(warp::path::param::<String>())
        .and(warp::path::end())
        .and(with_db(postgres.clone()))
        .and_then(handlers::user::get);

    let register = warp::post()
        .and(warp::path("register"))
        .and(warp::path::end())
        .and(warp::body::json())
        .and(with_settings(settings.clone()))
        .and(with_db(postgres.clone()))
        .and_then(handlers::user::register);

    let login = warp::post()
        .and(warp::path("login"))
        .and(warp::path::end())
        .and(warp::body::json())
        .and(with_db(postgres))
        .and_then(handlers::user::login);

    let r = warp::any()
        .and(
            index
                .or(count)
                .or(sync)
                .or(add_history)
                .or(user)
                .or(register)
                .or(login)
                .or(warp::any().map(|| warp::reply::with_status("☕", StatusCode::IM_A_TEAPOT))),
        )
        .with(warp::filters::log::log("atuin::api"));

    Ok(r)
}
