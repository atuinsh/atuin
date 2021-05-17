macro_rules! ffi_fn {
    ($(#[$doc:meta])* fn $name:ident($($arg:ident: $arg_ty:ty),*) -> $ret:ty $body:block ?= $default:expr) => {
        $(#[$doc])*
        #[no_mangle]
        pub extern fn $name($($arg: $arg_ty),*) -> $ret {
            use std::panic::{self, AssertUnwindSafe};

            match panic::catch_unwind(AssertUnwindSafe(move || $body)) {
                Ok(v) => v,
                Err(_) => {
                    $default
                }
            }
        }
    };

    ($(#[$doc:meta])* fn $name:ident($($arg:ident: $arg_ty:ty),*) -> $ret:ty $body:block) => {
        ffi_fn!($(#[$doc])* fn $name($($arg: $arg_ty),*) -> $ret $body ?= {
            eprintln!("panic unwind caught, aborting");
            std::process::abort()
        });
    };

    ($(#[$doc:meta])* fn $name:ident($($arg:ident: $arg_ty:ty),*) $body:block ?= $default:expr) => {
        ffi_fn!($(#[$doc])* fn $name($($arg: $arg_ty),*) -> () $body ?= $default);
    };

    ($(#[$doc:meta])* fn $name:ident($($arg:ident: $arg_ty:ty),*) $body:block) => {
        ffi_fn!($(#[$doc])* fn $name($($arg: $arg_ty),*) -> () $body);
    };
}
