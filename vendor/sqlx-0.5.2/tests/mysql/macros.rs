use sqlx::{Connection, MySql, MySqlConnection, Transaction};
use sqlx_test::new;

#[sqlx_macros::test]
async fn macro_select_from_cte() -> anyhow::Result<()> {
    let mut conn = new::<MySql>().await?;
    let account =
        sqlx::query!("select * from (select (1) as id, 'Herp Derpinson' as name, cast(null as char) email) accounts")
            .fetch_one(&mut conn)
            .await?;

    assert_eq!(account.id, 1);
    assert_eq!(account.name, "Herp Derpinson");
    // MySQL can tell us the nullability of expressions, ain't that cool
    assert_eq!(account.email, None);

    Ok(())
}

#[sqlx_macros::test]
async fn macro_select_from_cte_bind() -> anyhow::Result<()> {
    let mut conn = new::<MySql>().await?;
    let account = sqlx::query!(
        "select * from (select (1) as id, 'Herp Derpinson' as name) accounts where id = ?",
        1i32
    )
    .fetch_one(&mut conn)
    .await?;

    println!("{:?}", account);
    println!("{}: {}", account.id, account.name);

    Ok(())
}

#[derive(Debug)]
struct RawAccount {
    r#type: i32,
    name: Option<String>,
}

#[sqlx_macros::test]
async fn test_query_as_raw() -> anyhow::Result<()> {
    let mut conn = new::<MySql>().await?;

    let account = sqlx::query_as!(
        RawAccount,
        "SELECT * from (select 1 as type, cast(null as char) as name) accounts"
    )
    .fetch_one(&mut conn)
    .await?;

    assert_eq!(account.name, None);
    assert_eq!(account.r#type, 1);

    println!("{:?}", account);

    Ok(())
}

#[sqlx_macros::test]
async fn test_query_scalar() -> anyhow::Result<()> {
    let mut conn = new::<MySql>().await?;

    let id = sqlx::query_scalar!("select 1").fetch_one(&mut conn).await?;
    // MySQL tells us `LONG LONG` while MariaDB just `LONG`
    assert_eq!(id, 1);

    // invalid column names are ignored
    let id = sqlx::query_scalar!(r#"select 1 as `&foo`"#)
        .fetch_one(&mut conn)
        .await?;
    assert_eq!(id, 1);

    let id = sqlx::query_scalar!(r#"select 1 as `foo!`"#)
        .fetch_one(&mut conn)
        .await?;
    assert_eq!(id, 1);

    let id = sqlx::query_scalar!(r#"select 1 as `foo?`"#)
        .fetch_one(&mut conn)
        .await?;

    assert_eq!(id, Some(1));

    let id = sqlx::query_scalar!(r#"select 1 as `foo: MyInt`"#)
        .fetch_one(&mut conn)
        .await?;

    assert_eq!(id, MyInt(1));

    let id = sqlx::query_scalar!(r#"select 1 as `foo?: MyInt`"#)
        .fetch_one(&mut conn)
        .await?;

    assert_eq!(id, Some(MyInt(1)));

    let id = sqlx::query_scalar!(r#"select 1 as `foo!: MyInt`"#)
        .fetch_one(&mut conn)
        .await?;

    assert_eq!(id, MyInt(1));

    let id: MyInt = sqlx::query_scalar!(r#"select 1 as `foo: _`"#)
        .fetch_one(&mut conn)
        .await?;

    assert_eq!(id, MyInt(1));

    let id: MyInt = sqlx::query_scalar!(r#"select 1 as `foo?: _`"#)
        .fetch_one(&mut conn)
        .await?
        // don't hint that it should be `Option<MyInt>`
        .unwrap();

    assert_eq!(id, MyInt(1));

    let id: MyInt = sqlx::query_scalar!(r#"select 1 as `foo!: _`"#)
        .fetch_one(&mut conn)
        .await?;

    assert_eq!(id, MyInt(1));

    Ok(())
}

#[sqlx_macros::test]
async fn test_query_as_bool() -> anyhow::Result<()> {
    let mut conn = new::<MySql>().await?;

    struct Article {
        id: i32,
        deleted: bool,
    }

    let article = sqlx::query_as_unchecked!(
        Article,
        "select * from (select 51 as id, true as deleted) articles"
    )
    .fetch_one(&mut conn)
    .await?;

    assert_eq!(51, article.id);
    assert_eq!(true, article.deleted);

    Ok(())
}

#[sqlx_macros::test]
async fn test_query_bytes() -> anyhow::Result<()> {
    let mut conn = new::<MySql>().await?;

    let rec = sqlx::query!("SELECT X'01AF' as _1")
        .fetch_one(&mut conn)
        .await?;

    assert_eq!(rec._1, &[0x01_u8, 0xAF_u8]);

    Ok(())
}

#[sqlx_macros::test]
async fn test_column_override_not_null() -> anyhow::Result<()> {
    let mut conn = new::<MySql>().await?;

    let record = sqlx::query!("select * from (select 1 as `id!`) records")
        .fetch_one(&mut conn)
        .await?;

    assert_eq!(record.id, 1);

    Ok(())
}

#[sqlx_macros::test]
async fn test_column_override_nullable() -> anyhow::Result<()> {
    let mut conn = new::<MySql>().await?;

    // MySQL by default tells us `id` is not-null
    let record = sqlx::query!("select * from (select 1 as `id?`) records")
        .fetch_one(&mut conn)
        .await?;

    assert_eq!(record.id, Some(1));

    Ok(())
}

async fn with_test_row<'a>(
    conn: &'a mut MySqlConnection,
) -> anyhow::Result<Transaction<'a, MySql>> {
    let mut transaction = conn.begin().await?;
    sqlx::query!("INSERT INTO tweet(id, text, owner_id) VALUES (1, '#sqlx is pretty cool!', 1)")
        .execute(&mut transaction)
        .await?;
    Ok(transaction)
}

#[derive(PartialEq, Eq, Debug, sqlx::Type)]
#[sqlx(transparent)]
struct MyInt(i64);

struct Record {
    id: MyInt,
}

struct OptionalRecord {
    id: Option<MyInt>,
}

#[sqlx_macros::test]
async fn test_column_override_wildcard() -> anyhow::Result<()> {
    let mut conn = new::<MySql>().await?;
    let mut conn = with_test_row(&mut conn).await?;

    let record = sqlx::query_as!(Record, "select id as `id: _` from tweet")
        .fetch_one(&mut conn)
        .await?;

    assert_eq!(record.id, MyInt(1));

    // this syntax is also useful for expressions
    let record = sqlx::query_as!(Record, "select * from (select 1 as `id: _`) records")
        .fetch_one(&mut conn)
        .await?;

    assert_eq!(record.id, MyInt(1));

    let record = sqlx::query_as!(OptionalRecord, "select owner_id as `id: _` from tweet")
        .fetch_one(&mut conn)
        .await?;

    assert_eq!(record.id, Some(MyInt(1)));

    Ok(())
}

#[sqlx_macros::test]
async fn test_column_override_wildcard_not_null() -> anyhow::Result<()> {
    let mut conn = new::<MySql>().await?;
    let mut conn = with_test_row(&mut conn).await?;

    let record = sqlx::query_as!(Record, "select owner_id as `id!: _` from tweet")
        .fetch_one(&mut conn)
        .await?;

    assert_eq!(record.id, MyInt(1));

    Ok(())
}

#[sqlx_macros::test]
async fn test_column_override_wildcard_nullable() -> anyhow::Result<()> {
    let mut conn = new::<MySql>().await?;
    let mut conn = with_test_row(&mut conn).await?;

    let record = sqlx::query_as!(OptionalRecord, "select id as `id?: _` from tweet")
        .fetch_one(&mut conn)
        .await?;

    assert_eq!(record.id, Some(MyInt(1)));

    Ok(())
}

#[sqlx_macros::test]
async fn test_column_override_exact() -> anyhow::Result<()> {
    let mut conn = new::<MySql>().await?;
    let mut conn = with_test_row(&mut conn).await?;

    let record = sqlx::query!("select id as `id: MyInt` from tweet")
        .fetch_one(&mut conn)
        .await?;

    assert_eq!(record.id, MyInt(1));

    // we can also support this syntax for expressions
    let record = sqlx::query!("select * from (select 1 as `id: MyInt`) records")
        .fetch_one(&mut conn)
        .await?;

    assert_eq!(record.id, MyInt(1));

    let record = sqlx::query!("select owner_id as `id: MyInt` from tweet")
        .fetch_one(&mut conn)
        .await?;

    assert_eq!(record.id, Some(MyInt(1)));

    Ok(())
}

#[sqlx_macros::test]
async fn test_column_override_exact_not_null() -> anyhow::Result<()> {
    let mut conn = new::<MySql>().await?;
    let mut conn = with_test_row(&mut conn).await?;

    let record = sqlx::query!("select owner_id as `id!: MyInt` from tweet")
        .fetch_one(&mut conn)
        .await?;

    assert_eq!(record.id, MyInt(1));

    Ok(())
}

#[sqlx_macros::test]
async fn test_column_override_exact_nullable() -> anyhow::Result<()> {
    let mut conn = new::<MySql>().await?;
    let mut conn = with_test_row(&mut conn).await?;

    let record = sqlx::query!("select id as `id?: MyInt` from tweet")
        .fetch_one(&mut conn)
        .await?;

    assert_eq!(record.id, Some(MyInt(1)));

    Ok(())
}

#[derive(PartialEq, Eq, Debug, sqlx::Type)]
#[sqlx(rename_all = "lowercase")]
enum MyEnum {
    Red,
    Green,
    Blue,
}

#[derive(PartialEq, Eq, Debug, sqlx::Type)]
#[repr(i32)]
enum MyCEnum {
    Red = 0,
    Green,
    Blue,
}

#[sqlx_macros::test]
async fn test_column_override_exact_enum() -> anyhow::Result<()> {
    let mut conn = new::<MySql>().await?;

    let record = sqlx::query!("select * from (select 'red' as `color: MyEnum`) records")
        .fetch_one(&mut conn)
        .await?;

    assert_eq!(record.color, MyEnum::Red);

    let record = sqlx::query!("select * from (select 2 as `color: MyCEnum`) records")
        .fetch_one(&mut conn)
        .await?;

    assert_eq!(record.color, MyCEnum::Blue);

    Ok(())
}

// we don't emit bind parameter type-checks for MySQL so testing the overrides is redundant
