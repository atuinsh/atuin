use std::fmt;
use std::ops::{Deref, DerefMut};

use ntapi::ntioapi::IO_STATUS_BLOCK;

pub struct IoStatusBlock(IO_STATUS_BLOCK);

cfg_io_source! {
    use ntapi::ntioapi::IO_STATUS_BLOCK_u;

    impl IoStatusBlock {
        pub fn zeroed() -> Self {
            Self(IO_STATUS_BLOCK {
                u: IO_STATUS_BLOCK_u { Status: 0 },
                Information: 0,
            })
        }
    }
}

unsafe impl Send for IoStatusBlock {}

impl Deref for IoStatusBlock {
    type Target = IO_STATUS_BLOCK;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl DerefMut for IoStatusBlock {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

impl fmt::Debug for IoStatusBlock {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("IoStatusBlock").finish()
    }
}
