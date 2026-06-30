# atuin-server-database

The create `atuin-server-database` contains the abstraction for PostgreSQL,
SQLLite & MySQL.

## SQLx

SQLx is the Rust wrapper over the database providing an abstraction layer

TODO: Figure out how migration scripts are generated and document

## SQLx Migations

### Run pending

```shell
DATABASE_URL=mysql://root:foobar@127.0.0.1/atuin cargo sqlx migrate run
```

### Revert last

```shell
DATABASE_URL=mysql://root:foobar@127.0.0.1/atuin cargo sqlx migrate revert
```

### Reset the database and rerun all migrations

```shell
DATABASE_URL=mysql://root:foobar@127.0.0.1/atuin cargo sqlx database reset
```
