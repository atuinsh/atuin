use crate::arguments::Arguments;
use crate::encode::Encode;
use crate::mssql::database::Mssql;
use crate::mssql::io::MssqlBufMutExt;
use crate::mssql::protocol::rpc::StatusFlags;
use crate::types::Type;

#[derive(Default)]
pub struct MssqlArguments {
    // next ordinal to be used when formatting a positional parameter name
    pub(crate) ordinal: usize,
    // temporary string buffer used to format parameter names
    name: String,
    pub(crate) data: Vec<u8>,
    pub(crate) declarations: String,
}

impl MssqlArguments {
    pub(crate) fn add_named<'q, T: Encode<'q, Mssql> + Type<Mssql>>(
        &mut self,
        name: &str,
        value: T,
    ) {
        let ty = value.produces().unwrap_or_else(T::type_info);

        let mut ty_name = String::new();
        ty.0.fmt(&mut ty_name);

        self.data.put_b_varchar(name); // [ParamName]
        self.data.push(0); // [StatusFlags]

        ty.0.put(&mut self.data); // [TYPE_INFO]
        ty.0.put_value(&mut self.data, value); // [ParamLenData]
    }

    pub(crate) fn add_unnamed<'q, T: Encode<'q, Mssql> + Type<Mssql>>(&mut self, value: T) {
        self.add_named("", value);
    }

    pub(crate) fn declare<'q, T: Encode<'q, Mssql> + Type<Mssql>>(
        &mut self,
        name: &str,
        initial_value: T,
    ) {
        let ty = initial_value.produces().unwrap_or_else(T::type_info);

        let mut ty_name = String::new();
        ty.0.fmt(&mut ty_name);

        self.data.put_b_varchar(name); // [ParamName]
        self.data.push(StatusFlags::BY_REF_VALUE.bits()); // [StatusFlags]

        ty.0.put(&mut self.data); // [TYPE_INFO]
        ty.0.put_value(&mut self.data, initial_value); // [ParamLenData]
    }

    pub(crate) fn append(&mut self, arguments: &mut MssqlArguments) {
        self.ordinal += arguments.ordinal;
        self.data.append(&mut arguments.data);
    }

    pub(crate) fn add<'q, T>(&mut self, value: T)
    where
        T: Encode<'q, Mssql> + Type<Mssql>,
    {
        let ty = value.produces().unwrap_or_else(T::type_info);

        // produce an ordinal parameter name
        //  @p1, @p2, ... @pN

        self.name.clear();
        self.name.push_str("@p");

        self.ordinal += 1;
        let _ = itoa::fmt(&mut self.name, self.ordinal);

        let MssqlArguments {
            ref name,
            ref mut declarations,
            ref mut data,
            ..
        } = self;

        // add this to our variable declaration list
        //  @p1 int, @p2 nvarchar(10), ...

        if !declarations.is_empty() {
            declarations.push_str(",");
        }

        declarations.push_str(name);
        declarations.push(' ');
        ty.0.fmt(declarations);

        // write out the parameter

        data.put_b_varchar(name); // [ParamName]
        data.push(0); // [StatusFlags]

        ty.0.put(data); // [TYPE_INFO]
        ty.0.put_value(data, value); // [ParamLenData]
    }
}

impl<'q> Arguments<'q> for MssqlArguments {
    type Database = Mssql;

    fn reserve(&mut self, _additional: usize, size: usize) {
        self.data.reserve(size + 10); // est. 4 chars for name, 1 for status, 1 for TYPE_INFO
    }

    fn add<T>(&mut self, value: T)
    where
        T: 'q + Encode<'q, Self::Database> + Type<Mssql>,
    {
        self.add(value)
    }
}
