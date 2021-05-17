use crate::acquire::Acquire;
use crate::migrate::{AppliedMigration, Migrate, MigrateError, Migration, MigrationSource};
use std::borrow::Cow;
use std::collections::{HashMap, HashSet};
use std::ops::Deref;
use std::slice;

#[derive(Debug)]
pub struct Migrator {
    pub migrations: Cow<'static, [Migration]>,
    pub ignore_missing: bool,
}

fn validate_applied_migrations(
    applied_migrations: &[AppliedMigration],
    migrator: &Migrator,
) -> Result<(), MigrateError> {
    if migrator.ignore_missing {
        return Ok(());
    }

    let migrations: HashSet<_> = migrator.iter().map(|m| m.version).collect();

    for applied_migration in applied_migrations {
        if !migrations.contains(&applied_migration.version) {
            return Err(MigrateError::VersionMissing(applied_migration.version));
        }
    }

    Ok(())
}

impl Migrator {
    /// Creates a new instance with the given source.
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// # use sqlx_core::migrate::MigrateError;
    /// # fn main() -> Result<(), MigrateError> {
    /// # sqlx_rt::block_on(async move {
    /// # use sqlx_core::migrate::Migrator;
    /// use std::path::Path;
    ///
    /// // Read migrations from a local folder: ./migrations
    /// let m = Migrator::new(Path::new("./migrations")).await?;
    /// # Ok(())
    /// # })
    /// # }
    /// ```
    /// See [MigrationSource] for details on structure of the `./migrations` directory.
    pub async fn new<'s, S>(source: S) -> Result<Self, MigrateError>
    where
        S: MigrationSource<'s>,
    {
        Ok(Self {
            migrations: Cow::Owned(source.resolve().await.map_err(MigrateError::Source)?),
            ignore_missing: false,
        })
    }

    /// Specify should ignore applied migrations that missing in the resolved migrations.
    pub fn set_ignore_missing(&mut self, ignore_missing: bool) -> &Self {
        self.ignore_missing = ignore_missing;
        self
    }

    /// Get an iterator over all known migrations.
    pub fn iter(&self) -> slice::Iter<'_, Migration> {
        self.migrations.iter()
    }

    /// Run any pending migrations against the database; and, validate previously applied migrations
    /// against the current migration source to detect accidental changes in previously-applied migrations.
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// # use sqlx_core::migrate::MigrateError;
    /// # #[cfg(feature = "sqlite")]
    /// # fn main() -> Result<(), MigrateError> {
    /// #     sqlx_rt::block_on(async move {
    /// # use sqlx_core::migrate::Migrator;
    /// let m = Migrator::new(std::path::Path::new("./migrations")).await?;
    /// let pool = sqlx_core::sqlite::SqlitePoolOptions::new().connect("sqlite::memory:").await?;
    /// m.run(&pool).await
    /// #     })
    /// # }
    /// ```
    pub async fn run<'a, A>(&self, migrator: A) -> Result<(), MigrateError>
    where
        A: Acquire<'a>,
        <A::Connection as Deref>::Target: Migrate,
    {
        let mut conn = migrator.acquire().await?;

        // lock the database for exclusive access by the migrator
        conn.lock().await?;

        // creates [_migrations] table only if needed
        // eventually this will likely migrate previous versions of the table
        conn.ensure_migrations_table().await?;

        let version = conn.dirty_version().await?;
        if let Some(version) = version {
            return Err(MigrateError::Dirty(version));
        }

        let applied_migrations = conn.list_applied_migrations().await?;
        validate_applied_migrations(&applied_migrations, self)?;

        let applied_migrations: HashMap<_, _> = applied_migrations
            .into_iter()
            .map(|m| (m.version, m))
            .collect();

        for migration in self.iter() {
            if migration.migration_type.is_down_migration() {
                continue;
            }

            match applied_migrations.get(&migration.version) {
                Some(applied_migration) => {
                    if migration.checksum != applied_migration.checksum {
                        return Err(MigrateError::VersionMismatch(migration.version));
                    }
                }
                None => {
                    conn.apply(migration).await?;
                }
            }
        }

        // unlock the migrator to allow other migrators to run
        // but do nothing as we already migrated
        conn.unlock().await?;

        Ok(())
    }
}
