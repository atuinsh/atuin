use crate::io::Encode;
use crate::postgres::io::PgBufMutExt;
use crate::postgres::PgValueFormat;

#[derive(Debug)]
pub struct Bind<'a> {
    /// The ID of the destination portal (`None` selects the unnamed portal).
    pub portal: Option<u32>,

    /// The id of the source prepared statement.
    pub statement: u32,

    /// The parameter format codes. Each must presently be zero (text) or one (binary).
    ///
    /// There can be zero to indicate that there are no parameters or that the parameters all use the
    /// default format (text); or one, in which case the specified format code is applied to all
    /// parameters; or it can equal the actual number of parameters.
    pub formats: &'a [PgValueFormat],

    /// The number of parameters.
    pub num_params: i16,

    /// The value of each parameter, in the indicated format.
    pub params: &'a [u8],

    /// The result-column format codes. Each must presently be zero (text) or one (binary).
    ///
    /// There can be zero to indicate that there are no result columns or that the
    /// result columns should all use the default format (text); or one, in which
    /// case the specified format code is applied to all result columns (if any);
    /// or it can equal the actual number of result columns of the query.
    pub result_formats: &'a [PgValueFormat],
}

impl Encode<'_> for Bind<'_> {
    fn encode_with(&self, buf: &mut Vec<u8>, _: ()) {
        buf.push(b'B');

        buf.put_length_prefixed(|buf| {
            buf.put_portal_name(self.portal);

            buf.put_statement_name(self.statement);

            buf.extend(&(self.formats.len() as i16).to_be_bytes());

            for &format in self.formats {
                buf.extend(&(format as i16).to_be_bytes());
            }

            buf.extend(&self.num_params.to_be_bytes());

            buf.extend(self.params);

            buf.extend(&(self.result_formats.len() as i16).to_be_bytes());

            for &format in self.result_formats {
                buf.extend(&(format as i16).to_be_bytes());
            }
        });
    }
}

// TODO: Unit Test Bind
// TODO: Benchmark Bind
