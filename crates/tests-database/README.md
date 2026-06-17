# Database Integration Tests

tl;dr Run `cargo test` to exercise the SQLite database using a file in the temp directory

This is an integration test suite that runs through a simple "story" of creating a user, adding some history, adding some records and doing some deleting.  The idea is to ensure that a database implementation does the right thing.

## `ATUIN_TEST_DB_URI`
Setting this will create a database at that URI and then run through the tests.  Leaving it unset will create a sqlite db in $TMP

There will be a [snowflake_uid](https://docs.rs/snowflake_uid/latest/snowflake_uid/) appended to the end of the URL so the DB will be unique for the test

## Postgres

```shell
ATUIN_TEST_DB_URI=postgres://postgres:pg@localhost/atuin_test_ cargo test
```

Will create a database and run the tests on it.

If you want to quickly and easily run a postgres instance 

```shell
podman run --name atuin-pg-test -e POSTGRES_PASSWORD=pg -p 5432:5432 postgres
```

## MySQL

```shell
ATUIN_TEST_DB_URI=mysql://root:pass@localhost/atuin_test_ cargo test
```

Will create a database and run the tests on it.

If you want to quickly and easily run a MySQL instance 

```shell
podman run --name atuin-mysql-test -e MYSQL_ROOT_PASSWORD=pass -p 3306:3306 mysql
```

## Help, I want to see what was left over

Set the environment variable `ATUIN_TEST_DB_NO_DESTROY` to anything and run the tests, e.g.

```shell
ATUIN_TEST_DB_NO_DESTROY=1 ATUIN_TEST_DB_URI=postgres://postgres:pg@localhost/atuin_test_ cargo test
```

will give

```plain
postgres@127.0.0.1:postgres> \l
+--------------------------------+----------+----------+------------+------------+-----------------------+
| Name                           | Owner    | Encoding | Collate    | Ctype      | Access privileges     |
|--------------------------------+----------+----------+------------+------------+-----------------------|
| atuin_test_3734160738059550720 | postgres | UTF8     | en_US.utf8 | en_US.utf8 | <null>                |
```

## Design Decision
This is intentionally not an integration test in `atuin-server-database` because it calls into the database implementation crates and I did not want to introduce a circular dependency

## Glossary


| Term | Definition |
| :--- | :--- |
| user | You, yes you looking at this.  It's you! |
| session | a working session |
| record | ... | 
| history | not sure what the difference between this and record is |
| store | shrug |


