use std::io::Result;

use termcolor::BufferWriter;

pub(crate) trait Buffers {
    type Dimension;

    type Buffers;

    fn buffers(
        &self,
        writer: &BufferWriter,
        available_dimension: Self::Dimension,
    ) -> Result<Self::Buffers>;
}
