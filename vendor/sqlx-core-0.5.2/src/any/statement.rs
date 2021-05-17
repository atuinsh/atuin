use crate::any::{Any, AnyArguments, AnyColumn, AnyColumnIndex, AnyTypeInfo};
use crate::column::ColumnIndex;
use crate::error::Error;
use crate::ext::ustr::UStr;
use crate::statement::Statement;
use crate::HashMap;
use either::Either;
use std::borrow::Cow;
use std::sync::Arc;

pub struct AnyStatement<'q> {
    pub(crate) sql: Cow<'q, str>,
    pub(crate) parameters: Option<Either<Vec<AnyTypeInfo>, usize>>,
    pub(crate) column_names: Arc<HashMap<UStr, usize>>,
    pub(crate) columns: Vec<AnyColumn>,
}

impl<'q> Statement<'q> for AnyStatement<'q> {
    type Database = Any;

    fn to_owned(&self) -> AnyStatement<'static> {
        AnyStatement::<'static> {
            sql: Cow::Owned(self.sql.clone().into_owned()),
            column_names: self.column_names.clone(),
            parameters: self.parameters.clone(),
            columns: self.columns.clone(),
        }
    }

    fn sql(&self) -> &str {
        &self.sql
    }

    fn parameters(&self) -> Option<Either<&[AnyTypeInfo], usize>> {
        match &self.parameters {
            Some(Either::Left(types)) => Some(Either::Left(&types)),
            Some(Either::Right(count)) => Some(Either::Right(*count)),
            None => None,
        }
    }

    fn columns(&self) -> &[AnyColumn] {
        &self.columns
    }

    impl_statement_query!(AnyArguments<'_>);
}

impl<'i> ColumnIndex<AnyStatement<'_>> for &'i str
where
    &'i str: AnyColumnIndex,
{
    fn index(&self, statement: &AnyStatement<'_>) -> Result<usize, Error> {
        statement
            .column_names
            .get(*self)
            .ok_or_else(|| Error::ColumnNotFound((*self).into()))
            .map(|v| *v)
    }
}
