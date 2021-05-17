use std::ops::{Deref, DerefMut};

use crate::arguments::Arguments;
use crate::encode::{Encode, IsNull};
use crate::error::Error;
use crate::ext::ustr::UStr;
use crate::postgres::{PgConnection, PgTypeInfo, Postgres};
use crate::types::Type;

// TODO: buf.patch(|| ...) is a poor name, can we think of a better name? Maybe `buf.lazy(||)` ?
// TODO: Extend the patch system to support dynamic lengths
//       Considerations:
//          - The prefixed-len offset needs to be back-tracked and updated
//          - message::Bind needs to take a &PgArguments and use a `write` method instead of
//            referencing a buffer directly
//          - The basic idea is that we write bytes for the buffer until we get somewhere
//            that has a patch, we then apply the patch which should write to &mut Vec<u8>,
//            backtrack and update the prefixed-len, then write until the next patch offset

#[derive(Default)]
pub struct PgArgumentBuffer {
    buffer: Vec<u8>,

    // Number of arguments
    count: usize,

    // Whenever an `Encode` impl needs to defer some work until after we resolve parameter types
    // it can use `patch`.
    //
    // This currently is only setup to be useful if there is a *fixed-size* slot that needs to be
    // tweaked from the input type. However, that's the only use case we currently have.
    //
    patches: Vec<(
        usize, // offset
        usize, // argument index
        Box<dyn Fn(&mut [u8], &PgTypeInfo) + 'static + Send + Sync>,
    )>,

    // Whenever an `Encode` impl encounters a `PgTypeInfo` object that does not have an OID
    // It pushes a "hole" that must be patched later.
    //
    // The hole is a `usize` offset into the buffer with the type name that should be resolved
    // This is done for Records and Arrays as the OID is needed well before we are in an async
    // function and can just ask postgres.
    //
    type_holes: Vec<(usize, UStr)>, // Vec<{ offset, type_name }>
}

/// Implementation of [`Arguments`] for PostgreSQL.
#[derive(Default)]
pub struct PgArguments {
    // Types of each bind parameter
    pub(crate) types: Vec<PgTypeInfo>,

    // Buffer of encoded bind parameters
    pub(crate) buffer: PgArgumentBuffer,
}

impl PgArguments {
    pub(crate) fn add<'q, T>(&mut self, value: T)
    where
        T: Encode<'q, Postgres> + Type<Postgres>,
    {
        // remember the type information for this value
        self.types
            .push(value.produces().unwrap_or_else(T::type_info));

        // encode the value into our buffer
        self.buffer.encode(value);

        // increment the number of arguments we are tracking
        self.buffer.count += 1;
    }

    // Apply patches
    // This should only go out and ask postgres if we have not seen the type name yet
    pub(crate) async fn apply_patches(
        &mut self,
        conn: &mut PgConnection,
        parameters: &[PgTypeInfo],
    ) -> Result<(), Error> {
        let PgArgumentBuffer {
            ref patches,
            ref type_holes,
            ref mut buffer,
            ..
        } = self.buffer;

        for (offset, ty, callback) in patches {
            let buf = &mut buffer[*offset..];
            let ty = &parameters[*ty];

            callback(buf, ty);
        }

        for (offset, name) in type_holes {
            let oid = conn.fetch_type_id_by_name(&*name).await?;
            buffer[*offset..(*offset + 4)].copy_from_slice(&oid.to_be_bytes());
        }

        Ok(())
    }
}

impl<'q> Arguments<'q> for PgArguments {
    type Database = Postgres;

    fn reserve(&mut self, additional: usize, size: usize) {
        self.types.reserve(additional);
        self.buffer.reserve(size);
    }

    fn add<T>(&mut self, value: T)
    where
        T: Encode<'q, Self::Database> + Type<Self::Database>,
    {
        self.add(value)
    }
}

impl PgArgumentBuffer {
    pub(crate) fn encode<'q, T>(&mut self, value: T)
    where
        T: Encode<'q, Postgres>,
    {
        // reserve space to write the prefixed length of the value
        let offset = self.len();
        self.extend(&[0; 4]);

        // encode the value into our buffer
        let len = if let IsNull::No = value.encode(self) {
            (self.len() - offset - 4) as i32
        } else {
            // Write a -1 to indicate NULL
            // NOTE: It is illegal for [encode] to write any data
            debug_assert_eq!(self.len(), offset + 4);
            -1_i32
        };

        // write the len to the beginning of the value
        self[offset..(offset + 4)].copy_from_slice(&len.to_be_bytes());
    }

    // Adds a callback to be invoked later when we know the parameter type
    #[allow(dead_code)]
    pub(crate) fn patch<F>(&mut self, callback: F)
    where
        F: Fn(&mut [u8], &PgTypeInfo) + 'static + Send + Sync,
    {
        let offset = self.len();
        let index = self.count;

        self.patches.push((offset, index, Box::new(callback)));
    }

    // Extends the inner buffer by enough space to have an OID
    // Remembers where the OID goes and type name for the OID
    pub(crate) fn patch_type_by_name(&mut self, type_name: &UStr) {
        let offset = self.len();

        self.extend_from_slice(&0_u32.to_be_bytes());
        self.type_holes.push((offset, type_name.clone()));
    }
}

impl Deref for PgArgumentBuffer {
    type Target = Vec<u8>;

    #[inline]
    fn deref(&self) -> &Self::Target {
        &self.buffer
    }
}

impl DerefMut for PgArgumentBuffer {
    #[inline]
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.buffer
    }
}
