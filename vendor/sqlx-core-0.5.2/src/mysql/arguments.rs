use crate::arguments::Arguments;
use crate::encode::{Encode, IsNull};
use crate::mysql::{MySql, MySqlTypeInfo};
use crate::types::Type;

/// Implementation of [`Arguments`] for MySQL.
#[derive(Debug, Default)]
pub struct MySqlArguments {
    pub(crate) values: Vec<u8>,
    pub(crate) types: Vec<MySqlTypeInfo>,
    pub(crate) null_bitmap: Vec<u8>,
}

impl MySqlArguments {
    pub(crate) fn add<'q, T>(&mut self, value: T)
    where
        T: Encode<'q, MySql> + Type<MySql>,
    {
        let ty = value.produces().unwrap_or_else(T::type_info);
        let index = self.types.len();

        self.types.push(ty);
        self.null_bitmap.resize((index / 8) + 1, 0);

        if let IsNull::Yes = value.encode(&mut self.values) {
            self.null_bitmap[index / 8] |= (1 << (index % 8)) as u8;
        }
    }
}

impl<'q> Arguments<'q> for MySqlArguments {
    type Database = MySql;

    fn reserve(&mut self, len: usize, size: usize) {
        self.types.reserve(len);
        self.values.reserve(size);
    }

    fn add<T>(&mut self, value: T)
    where
        T: Encode<'q, Self::Database> + Type<Self::Database>,
    {
        self.add(value)
    }
}
