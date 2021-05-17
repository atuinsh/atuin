use sqlx_core::database::Database;

#[derive(PartialEq, Eq)]
#[allow(dead_code)]
pub enum ParamChecking {
    Strong,
    Weak,
}

pub trait DatabaseExt: Database {
    const DATABASE_PATH: &'static str;
    const ROW_PATH: &'static str;
    const NAME: &'static str;

    const PARAM_CHECKING: ParamChecking;

    fn db_path() -> syn::Path {
        syn::parse_str(Self::DATABASE_PATH).unwrap()
    }

    fn row_path() -> syn::Path {
        syn::parse_str(Self::ROW_PATH).unwrap()
    }

    fn param_type_for_id(id: &Self::TypeInfo) -> Option<&'static str>;

    fn return_type_for_id(id: &Self::TypeInfo) -> Option<&'static str>;

    fn get_feature_gate(info: &Self::TypeInfo) -> Option<&'static str>;
}

macro_rules! impl_database_ext {
    (
        $database:path {
            $($(#[$meta:meta])? $ty:ty $(| $input:ty)?),*$(,)?
        },
        ParamChecking::$param_checking:ident,
        feature-types: $ty_info:ident => $get_gate:expr,
        row = $row:path,
        name = $db_name:literal
    ) => {
        impl $crate::database::DatabaseExt for $database {
            const DATABASE_PATH: &'static str = stringify!($database);
            const ROW_PATH: &'static str = stringify!($row);
            const PARAM_CHECKING: $crate::database::ParamChecking = $crate::database::ParamChecking::$param_checking;
            const NAME: &'static str = $db_name;

            fn param_type_for_id(info: &Self::TypeInfo) -> Option<&'static str> {
                match () {
                    $(
                        $(#[$meta])?
                        _ if <$ty as sqlx_core::types::Type<$database>>::type_info() == *info => Some(input_ty!($ty $(, $input)?)),
                    )*
                    $(
                        $(#[$meta])?
                        _ if <$ty as sqlx_core::types::Type<$database>>::compatible(info) => Some(input_ty!($ty $(, $input)?)),
                    )*
                    _ => None
                }
            }

            fn return_type_for_id(info: &Self::TypeInfo) -> Option<&'static str> {
                match () {
                    $(
                        $(#[$meta])?
                        _ if <$ty as sqlx_core::types::Type<$database>>::type_info() == *info => return Some(stringify!($ty)),
                    )*
                    $(
                        $(#[$meta])?
                        _ if <$ty as sqlx_core::types::Type<$database>>::compatible(info) => return Some(stringify!($ty)),
                    )*
                    _ => None
                }
            }

            fn get_feature_gate($ty_info: &Self::TypeInfo) -> Option<&'static str> {
                $get_gate
            }
        }
    }
}

macro_rules! input_ty {
    ($ty:ty, $input:ty) => {
        stringify!($input)
    };
    ($ty:ty) => {
        stringify!($ty)
    };
}

#[cfg(feature = "postgres")]
mod postgres;

#[cfg(feature = "mysql")]
mod mysql;

#[cfg(feature = "sqlite")]
mod sqlite;

#[cfg(feature = "mssql")]
mod mssql;
