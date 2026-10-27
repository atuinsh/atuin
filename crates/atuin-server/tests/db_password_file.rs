use atuin_server::db::DbSettings;
use rstest::rstest;
use url::Url;

/// The password reaches Postgres through `db_password_file` alone: the same `db_uri` without the
/// file is refused, so a server that ignored the file could not pass.
#[rstest]
#[tokio::test]
async fn postgres_authenticates_with_the_password_file() {
    let db_uri = atuin_common::env::var("ATUIN_DB_URI")
        .unwrap_or_else(|_| "postgres://atuin:pass@localhost:5432/atuin".to_owned());
    let mut url = Url::parse(&db_uri).unwrap();
    let password = url.password().expect("ATUIN_DB_URI holds a password").to_owned();
    url.set_password(None).unwrap();

    let file = tempfile::NamedTempFile::new().unwrap();
    fs_err::write(file.path(), format!("{password}\n")).unwrap();

    let without_file = DbSettings {
        db_uri: url.as_str().parse().unwrap(),
        db_password_file: None,
    };
    assert!(
        atuin_server::connect(without_file.connect_url().unwrap()).await.is_err(),
        "Postgres accepted {url} with no password, so this test cannot tell the file was used"
    );

    let with_file = DbSettings {
        db_password_file: Some(file.path().to_owned()),
        ..without_file
    };
    atuin_server::connect(with_file.connect_url().unwrap()).await.unwrap();
}
