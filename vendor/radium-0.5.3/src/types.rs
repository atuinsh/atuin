//! Best-effort atomic types
//!
//! This module exports `RadiumType` aliases that map to the `AtomicType` on
//! targets that have it, or `Cell<Type>` on targets that do not. This alias can
//! be used as a consistent name for crates that need consistent names to
//! portable types.

macro_rules! radium_type {
    ($flag:ident $( $name:ident $atom:ident => $inner:ty => $adoc:literal $cdoc:literal )*) => { $(
        #[doc = $adoc]
        #[cfg($flag)]
        pub type $name = core::sync::atomic::$atom;

        #[doc = $cdoc]
        #[cfg(not($flag))]
        pub type $name = core::cell::Cell<$inner>;
    )* };
}

radium_type!(radium_atomic_8
    RadiumBool AtomicBool => bool => "`AtomicBool`" "`Cell<bool>`"
    RadiumI8 AtomicI8 => i8 => "`AtomicI8`" "`Cell<i8>`"
    RadiumU8 AtomicU8 => u8 => "`AtomicU8`" "`Cell<u8>`"
);

radium_type!(radium_atomic_16
    RadiumI16 AtomicI16 => i16 => "`AtomicI16`" "`Cell<i16>`"
    RadiumU16 AtomicU16 => u16 => "`AtomicU16`" "`Cell<u16>`"
);

radium_type!(radium_atomic_32
    RadiumI32 AtomicI32 => i32 => "`AtomicI32`" "`Cell<i32>`"
    RadiumU32 AtomicU32 => u32 => "`AtomicU32`" "`Cell<u32>`"
);

radium_type!(radium_atomic_64
    RadiumI64 AtomicI64 => i64 => "`AtomicI64`" "`Cell<i64>`"
    RadiumU64 AtomicU64 => u64 => "`AtomicU64`" "`Cell<u64>`"
);

radium_type!(radium_atomic_ptr
    RadiumIsize AtomicIsize => isize => "`AtomicIsize`" "`Cell<isize>`"
    RadiumUsize AtomicUsize => usize => "`AtomicUsize`" "`Cell<usize>`"
);

/// `AtomicPtr`
#[cfg(radium_atomic_ptr)]
pub type RadiumPtr<T> = core::sync::atomic::AtomicPtr<T>;

/// `Cell<*mut T>`
#[cfg(not(radium_atomic_ptr))]
pub type RadiumPtr<T> = core::cell::Cell<*mut T>;
