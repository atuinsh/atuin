use atuin_common::utils::uuid_v7;
use rstest::{fixture, rstest};

mod common;

type TestServer = (url::Url, tokio::sync::oneshot::Sender<()>, tokio::task::JoinHandle<()>);

#[fixture]
async fn server() -> TestServer {
    let path = format!("/{}", uuid_v7().as_simple());
    common::start_server(&path).await
}

#[rstest]
#[tokio::test]
async fn registration(#[future] server: TestServer) {
    let (address, shutdown, server_task) = server.await;
    dbg!(&address);

    // -- REGISTRATION --

    let username = uuid_v7().as_simple().to_string();
    let password = uuid_v7().as_simple().to_string();
    let client = common::register_inner(&address, &username, &password).await;

    // the session token works
    let status = client.me().await.unwrap();
    assert_eq!(status.username, username);

    // -- LOGIN --

    let client = common::login(&address, username.clone(), password).await;

    // the session token works
    let status = client.me().await.unwrap();
    assert_eq!(status.username, username);

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
    let status = client.me().await.unwrap();
    assert_eq!(status.username, username);

    // -- PASSWORD CHANGE --

    let current_password = password;
    let new_password = uuid_v7().as_simple().to_string();
    let result = client.change_password(current_password, new_password.clone()).await;

    // the password change request succeeded
    assert!(result.is_ok());

    // -- LOGIN --

    let client = common::login(&address, username.clone(), new_password).await;

    // login with new password yields a working token
    let status = client.me().await.unwrap();
    assert_eq!(status.username, username);

    shutdown.send(()).unwrap();
    server_task.await.unwrap();
}

#[rstest]
#[tokio::test]
async fn multi_user_test(#[future] server: TestServer) {
    let (address, shutdown, server_task) = server.await;
    dbg!(&address);

    // -- REGISTRATION --

    let user_one = uuid_v7().as_simple().to_string();
    let password_one = uuid_v7().as_simple().to_string();
    let client_one = common::register_inner(&address, &user_one, &password_one).await;

    // the session token works
    let status = client_one.me().await.unwrap();
    assert_eq!(status.username, user_one);

    let user_two = uuid_v7().as_simple().to_string();
    let password_two = uuid_v7().as_simple().to_string();
    let client_two = common::register_inner(&address, &user_two, &password_two).await;

    // the session token works
    let status = client_two.me().await.unwrap();
    assert_eq!(status.username, user_two);

    // check that we can change user one's password, and _this does not affect user two_

    let current_password = password_one;
    let new_password = uuid_v7().as_simple().to_string();
    let result = client_one.change_password(current_password, new_password.clone()).await;

    // the password change request succeeded
    assert!(result.is_ok());

    // -- LOGIN --

    let client_one = common::login(&address, user_one.clone(), new_password).await;
    let client_two = common::login(&address, user_two.clone(), password_two).await;

    // login with new password yields a working token
    let status = client_one.me().await.unwrap();
    assert_eq!(status.username, user_one);
    assert_ne!(status.username, user_two);

    let status = client_two.me().await.unwrap();
    assert_eq!(status.username, user_two);

    shutdown.send(()).unwrap();
    server_task.await.unwrap();
}
