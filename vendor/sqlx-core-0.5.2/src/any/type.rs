// Type is required by the bounds of the [`Row`] and [`Arguments`] trait but its been overridden in
// AnyRow and AnyArguments to not use this implementation; but instead, delegate to the
// database-specific implementation.
//
// The other use of this trait is for compile-time verification which is not feasible to support
// for the [`Any`] driver.
macro_rules! impl_any_type {
    ($ty:ty) => {
        impl crate::types::Type<crate::any::Any> for $ty {
            fn type_info() -> crate::any::AnyTypeInfo {
                // FIXME: nicer panic explaining why this isn't possible
                unimplemented!()
            }

            fn compatible(ty: &crate::any::AnyTypeInfo) -> bool {
                match &ty.0 {
                    #[cfg(feature = "postgres")]
                    crate::any::type_info::AnyTypeInfoKind::Postgres(ty) => {
                        <$ty as crate::types::Type<crate::postgres::Postgres>>::compatible(&ty)
                    }

                    #[cfg(feature = "mysql")]
                    crate::any::type_info::AnyTypeInfoKind::MySql(ty) => {
                        <$ty as crate::types::Type<crate::mysql::MySql>>::compatible(&ty)
                    }

                    #[cfg(feature = "sqlite")]
                    crate::any::type_info::AnyTypeInfoKind::Sqlite(ty) => {
                        <$ty as crate::types::Type<crate::sqlite::Sqlite>>::compatible(&ty)
                    }

                    #[cfg(feature = "mssql")]
                    crate::any::type_info::AnyTypeInfoKind::Mssql(ty) => {
                        <$ty as crate::types::Type<crate::mssql::Mssql>>::compatible(&ty)
                    }
                }
            }
        }
    };
}
