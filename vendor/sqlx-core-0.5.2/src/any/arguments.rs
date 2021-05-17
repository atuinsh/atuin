use crate::any::Any;
use crate::arguments::Arguments;
use crate::encode::Encode;
use crate::types::Type;

#[derive(Default)]
pub struct AnyArguments<'q> {
    values: Vec<Box<dyn Encode<'q, Any> + Send + 'q>>,
}

impl<'q> Arguments<'q> for AnyArguments<'q> {
    type Database = Any;

    fn reserve(&mut self, additional: usize, _size: usize) {
        self.values.reserve(additional);
    }

    fn add<T>(&mut self, value: T)
    where
        T: 'q + Send + Encode<'q, Self::Database> + Type<Self::Database>,
    {
        self.values.push(Box::new(value));
    }
}

pub struct AnyArgumentBuffer<'q>(pub(crate) AnyArgumentBufferKind<'q>);

pub(crate) enum AnyArgumentBufferKind<'q> {
    #[cfg(feature = "postgres")]
    Postgres(
        crate::postgres::PgArguments,
        std::marker::PhantomData<&'q ()>,
    ),

    #[cfg(feature = "mysql")]
    MySql(
        crate::mysql::MySqlArguments,
        std::marker::PhantomData<&'q ()>,
    ),

    #[cfg(feature = "sqlite")]
    Sqlite(crate::sqlite::SqliteArguments<'q>),

    #[cfg(feature = "mssql")]
    Mssql(
        crate::mssql::MssqlArguments,
        std::marker::PhantomData<&'q ()>,
    ),
}

// control flow inferred type bounds would be fun
// the compiler should know the branch is totally unreachable

#[cfg(feature = "sqlite")]
#[allow(irrefutable_let_patterns)]
impl<'q> From<AnyArguments<'q>> for crate::sqlite::SqliteArguments<'q> {
    fn from(args: AnyArguments<'q>) -> Self {
        let mut buf = AnyArgumentBuffer(AnyArgumentBufferKind::Sqlite(Default::default()));

        for value in args.values {
            let _ = value.encode_by_ref(&mut buf);
        }

        if let AnyArgumentBufferKind::Sqlite(args) = buf.0 {
            args
        } else {
            unreachable!()
        }
    }
}

#[cfg(feature = "mysql")]
#[allow(irrefutable_let_patterns)]
impl<'q> From<AnyArguments<'q>> for crate::mysql::MySqlArguments {
    fn from(args: AnyArguments<'q>) -> Self {
        let mut buf = AnyArgumentBuffer(AnyArgumentBufferKind::MySql(
            Default::default(),
            std::marker::PhantomData,
        ));

        for value in args.values {
            let _ = value.encode_by_ref(&mut buf);
        }

        if let AnyArgumentBufferKind::MySql(args, _) = buf.0 {
            args
        } else {
            unreachable!()
        }
    }
}

#[cfg(feature = "mssql")]
#[allow(irrefutable_let_patterns)]
impl<'q> From<AnyArguments<'q>> for crate::mssql::MssqlArguments {
    fn from(args: AnyArguments<'q>) -> Self {
        let mut buf = AnyArgumentBuffer(AnyArgumentBufferKind::Mssql(
            Default::default(),
            std::marker::PhantomData,
        ));

        for value in args.values {
            let _ = value.encode_by_ref(&mut buf);
        }

        if let AnyArgumentBufferKind::Mssql(args, _) = buf.0 {
            args
        } else {
            unreachable!()
        }
    }
}

#[cfg(feature = "postgres")]
#[allow(irrefutable_let_patterns)]
impl<'q> From<AnyArguments<'q>> for crate::postgres::PgArguments {
    fn from(args: AnyArguments<'q>) -> Self {
        let mut buf = AnyArgumentBuffer(AnyArgumentBufferKind::Postgres(
            Default::default(),
            std::marker::PhantomData,
        ));

        for value in args.values {
            let _ = value.encode_by_ref(&mut buf);
        }

        if let AnyArgumentBufferKind::Postgres(args, _) = buf.0 {
            args
        } else {
            unreachable!()
        }
    }
}
