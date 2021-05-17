use sqlx_core as sqlx;

impl_database_ext! {
    sqlx::mssql::Mssql {
        bool,
        i8,
        i16,
        i32,
        i64,
        f32,
        f64,
        String,
    },
    ParamChecking::Weak,
    feature-types: _info => None,
    row = sqlx::mssql::MssqlRow,
    name = "MSSQL"
}
