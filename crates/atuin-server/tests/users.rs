use atuin_api_client::{ApiError, MapApiError, types};
use atuin_common::utils::uuid_v7;
use reqwest::StatusCode;
use rstest::{fixture, rstest};

mod common;

type TestServer = (url::Url, tokio::sync::oneshot::Sender<()>, tokio::task::JoinHandle<()>);

#[fixture]
async fn server() -> TestServer {
    let path = format!("/{}", uuid_v7().as_simple());
    common::start_server(&path).await
}

/// The status and reason `err` reports, if the server answered with one.
fn refusal(err: ApiError) -> (Option<StatusCode>, Option<String>) {
    match err {
        ApiError::Status { status, reason, .. } => (Some(status), reason),
        ApiError::Transport(_) | ApiError::Decode(_) | ApiError::NotSent(_) => (None, None),
    }
}

#[rstest]
#[tokio::test]
async fn registration(#[future] server: TestServer) {
    let (address, shutdown, server_task) = server.await;

    // -- REGISTRATION --

    let username = uuid_v7().as_simple().to_string();
    let password = uuid_v7().as_simple().to_string();
    let client = common::register_inner(&address, &username, &password).await;

    // the session token works
    assert_eq!(common::username(&client).await, Some(username.clone()));

    // -- LOGIN --

    let client = common::login(&address, username.clone(), password).await;

    // the session token works
    assert_eq!(common::username(&client).await, Some(username));

    shutdown.send(()).unwrap();
    server_task.await.unwrap();
}

#[rstest]
#[tokio::test]
async fn change_password(#[future] server: TestServer) {
    let (address, shutdown, server_task) = server.await;

    // -- REGISTRATION --

    let username = uuid_v7().as_simple().to_string();
    let password = uuid_v7().as_simple().to_string();
    let client = common::register_inner(&address, &username, &password).await;

    // the session token works
    assert_eq!(common::username(&client).await, Some(username.clone()));

    // -- PASSWORD CHANGE --

    let new_password = uuid_v7().as_simple().to_string();
    let change = types::ChangePasswordRequest {
        current_password: password.into(),
        new_password: new_password.clone().into(),
        totp_code: None,
    };
    client.legacy_change_password(&change).map_api_error().await.unwrap();

    // -- LOGIN --

    let client = common::login(&address, username.clone(), new_password).await;

    // login with new password yields a working token
    assert_eq!(common::username(&client).await, Some(username));

    shutdown.send(()).unwrap();
    server_task.await.unwrap();
}

#[rstest]
#[tokio::test]
async fn multi_user_test(#[future] server: TestServer) {
    let (address, shutdown, server_task) = server.await;

    // -- REGISTRATION --

    let user_one = uuid_v7().as_simple().to_string();
    let password_one = uuid_v7().as_simple().to_string();
    let client_one = common::register_inner(&address, &user_one, &password_one).await;

    // the session token works
    assert_eq!(common::username(&client_one).await, Some(user_one.clone()));

    let user_two = uuid_v7().as_simple().to_string();
    let password_two = uuid_v7().as_simple().to_string();
    let client_two = common::register_inner(&address, &user_two, &password_two).await;

    // the session token works
    assert_eq!(common::username(&client_two).await, Some(user_two.clone()));

    // check that we can change user one's password, and _this does not affect user two_

    let new_password = uuid_v7().as_simple().to_string();
    let change = types::ChangePasswordRequest {
        current_password: password_one.into(),
        new_password: new_password.clone().into(),
        totp_code: None,
    };
    client_one.legacy_change_password(&change).map_api_error().await.unwrap();

    // -- LOGIN --

    let client_one = common::login(&address, user_one.clone(), new_password).await;
    let client_two = common::login(&address, user_two.clone(), password_two).await;

    // login with new password yields a working token
    assert_eq!(common::username(&client_one).await, Some(user_one));
    assert_eq!(common::username(&client_two).await, Some(user_two));

    shutdown.send(()).unwrap();
    server_task.await.unwrap();
}

/// `atuin register` treats any 2xx to the lookup as a taken username, so an unknown one must 404.
#[rstest]
#[tokio::test]
async fn user_lookup_finds_only_registered_usernames(#[future] server: TestServer) {
    let (address, shutdown, server_task) = server.await;
    let username = uuid_v7().as_simple().to_string();
    common::register_inner(&address, &username, "pw").await;
    let anonymous = common::client(&address, None);

    let found = anonymous.legacy_get_user(&username).map_api_error().await.unwrap().into_inner();
    let missing = anonymous.legacy_get_user("nobody").map_api_error().await.unwrap_err();

    assert_eq!(found.username, username);
    assert_eq!(refusal(missing), (Some(StatusCode::NOT_FOUND), Some("user not found".to_owned())));

    shutdown.send(()).unwrap();
    server_task.await.unwrap();
}

#[rstest]
#[tokio::test]
async fn delete_account_ends_the_session(#[future] server: TestServer) {
    let (address, shutdown, server_task) = server.await;
    let username = uuid_v7().as_simple().to_string();
    let client = common::register_inner(&address, &username, "pw").await;

    let body = types::DeleteUserRequest {
        password: "pw".into(),
        totp_code: None,
    };
    client.legacy_delete_account(&body).map_api_error().await.unwrap();
    let after = client.get_me().map_api_error().await.unwrap_err();

    assert_eq!(refusal(after), (Some(StatusCode::FORBIDDEN), Some("session not found".to_owned())));

    shutdown.send(()).unwrap();
    server_task.await.unwrap();
}

#[rstest]
#[tokio::test]
async fn capabilities_describe_the_server(#[future] server: TestServer) {
    let (address, shutdown, server_task) = server.await;

    let capabilities = common::client(&address, None).get_capabilities().map_api_error().await;

    assert!(!capabilities.unwrap().into_inner().version.is_empty());

    shutdown.send(()).unwrap();
    server_task.await.unwrap();
}
