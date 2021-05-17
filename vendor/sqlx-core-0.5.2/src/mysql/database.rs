use crate::database::{Database, HasArguments, HasStatement, HasStatementCache, HasValueRef};
use crate::mysql::value::{MySqlValue, MySqlValueRef};
use crate::mysql::{
    MySqlArguments, MySqlColumn, MySqlConnection, MySqlQueryResult, MySqlRow, MySqlStatement,
    MySqlTransactionManager, MySqlTypeInfo,
};

/// MySQL database driver.
#[derive(Debug)]
pub struct MySql;

impl Database for MySql {
    type Connection = MySqlConnection;

    type TransactionManager = MySqlTransactionManager;

    type Row = MySqlRow;

    type QueryResult = MySqlQueryResult;

    type Column = MySqlColumn;

    type TypeInfo = MySqlTypeInfo;

    type Value = MySqlValue;
}

impl<'r> HasValueRef<'r> for MySql {
    type Database = MySql;

    type ValueRef = MySqlValueRef<'r>;
}

impl HasArguments<'_> for MySql {
    type Database = MySql;

    type Arguments = MySqlArguments;

    type ArgumentBuffer = Vec<u8>;
}

impl<'q> HasStatement<'q> for MySql {
    type Database = MySql;

    type Statement = MySqlStatement<'q>;
}

impl HasStatementCache for MySql {}
