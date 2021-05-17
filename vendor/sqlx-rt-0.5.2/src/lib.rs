#[cfg(not(any(
    feature = "runtime-actix-native-tls",
    feature = "runtime-async-std-native-tls",
    feature = "runtime-tokio-native-tls",
    feature = "runtime-actix-rustls",
    feature = "runtime-async-std-rustls",
    feature = "runtime-tokio-rustls",
)))]
compile_error!(
    "one of the features ['runtime-actix-native-tls', 'runtime-async-std-native-tls', \
     'runtime-tokio-native-tls', 'runtime-actix-rustls', 'runtime-async-std-rustls', \
     'runtime-tokio-rustls'] must be enabled"
);

#[cfg(any(
    all(feature = "_rt-actix", feature = "_rt-async-std"),
    all(feature = "_rt-actix", feature = "_rt-tokio"),
    all(feature = "_rt-async-std", feature = "_rt-tokio"),
    all(feature = "_tls-native-tls", feature = "_tls-rustls"),
))]
compile_error!(
    "only one of ['runtime-actix-native-tls', 'runtime-async-std-native-tls', \
     'runtime-tokio-native-tls', 'runtime-actix-rustls', 'runtime-async-std-rustls', \
     'runtime-tokio-rustls'] can be enabled"
);

#[cfg(all(feature = "_tls-native-tls"))]
pub use native_tls;

//
// Actix *OR* Tokio
//

#[cfg(all(
    any(feature = "_rt-tokio", feature = "_rt-actix"),
    not(feature = "_rt-async-std"),
))]
pub use tokio::{
    self, fs, io::AsyncRead, io::AsyncReadExt, io::AsyncWrite, io::AsyncWriteExt, io::ReadBuf,
    net::TcpStream, task::spawn, task::yield_now, time::sleep, time::timeout,
};

#[cfg(all(
    unix,
    any(feature = "_rt-tokio", feature = "_rt-actix"),
    not(feature = "_rt-async-std"),
))]
pub use tokio::net::UnixStream;

#[cfg(all(
    any(feature = "_rt-tokio", feature = "_rt-actix"),
    not(feature = "_rt-async-std"),
))]
pub use tokio_runtime::{block_on, enter_runtime};

#[cfg(any(feature = "_rt-tokio", feature = "_rt-actix"))]
mod tokio_runtime {
    use once_cell::sync::Lazy;
    use tokio::runtime::{self, Runtime};

    // lazily initialize a global runtime once for multiple invocations of the macros
    static RUNTIME: Lazy<Runtime> = Lazy::new(|| {
        runtime::Builder::new_multi_thread()
            .enable_io()
            .enable_time()
            .build()
            .expect("failed to initialize Tokio runtime")
    });

    pub fn block_on<F: std::future::Future>(future: F) -> F::Output {
        RUNTIME.block_on(future)
    }

    pub fn enter_runtime<F, R>(f: F) -> R
    where
        F: FnOnce() -> R,
    {
        let _rt = RUNTIME.enter();
        f()
    }
}

#[cfg(all(
    feature = "_tls-native-tls",
    any(feature = "_rt-tokio", feature = "_rt-actix"),
    not(any(feature = "_tls-rustls", feature = "_rt-async-std")),
))]
pub use tokio_native_tls::{TlsConnector, TlsStream};

#[cfg(all(
    feature = "_tls-rustls",
    any(feature = "_rt-tokio", feature = "_rt-actix"),
    not(any(feature = "_tls-native-tls", feature = "_rt-async-std")),
))]
pub use tokio_rustls::{client::TlsStream, TlsConnector};

//
// tokio
//

#[cfg(all(
    feature = "_rt-tokio",
    not(any(feature = "_rt-actix", feature = "_rt-async-std")),
))]
#[macro_export]
macro_rules! blocking {
    ($($expr:tt)*) => {
        $crate::tokio::task::block_in_place(move || { $($expr)* })
    };
}

//
// actix
//

#[cfg(feature = "_rt-actix")]
pub use actix_rt;

#[cfg(all(
    feature = "_rt-actix",
    not(any(feature = "_rt-tokio", feature = "_rt-async-std")),
))]
#[macro_export]
macro_rules! blocking {
    ($($expr:tt)*) => {
         // spawn_blocking is a re-export from tokio
         $crate::actix_rt::task::spawn_blocking(move || { $($expr)* })
            .await
            .expect("Blocking task failed to complete.")
    };
}

//
// async-std
//

#[cfg(all(
    feature = "_rt-async-std",
    not(any(feature = "_rt-actix", feature = "_rt-tokio")),
))]
pub use async_std::{
    self, fs, future::timeout, io::prelude::ReadExt as AsyncReadExt,
    io::prelude::WriteExt as AsyncWriteExt, io::Read as AsyncRead, io::Write as AsyncWrite,
    net::TcpStream, task::sleep, task::spawn, task::yield_now,
};

#[cfg(all(
    feature = "_rt-async-std",
    not(any(feature = "_rt-actix", feature = "_rt-tokio")),
))]
#[macro_export]
macro_rules! blocking {
    ($($expr:tt)*) => {
        $crate::async_std::task::spawn_blocking(move || { $($expr)* }).await
    };
}

#[cfg(all(
    unix,
    feature = "_rt-async-std",
    not(any(feature = "_rt-actix", feature = "_rt-tokio")),
))]
pub use async_std::os::unix::net::UnixStream;

#[cfg(all(
    feature = "_rt-async-std",
    not(any(feature = "_rt-actix", feature = "_rt-tokio")),
))]
pub use async_std::task::block_on;

#[cfg(all(
    feature = "_rt-async-std",
    not(any(feature = "_rt-actix", feature = "_rt-tokio")),
))]
pub fn enter_runtime<F, R>(f: F) -> R
where
    F: FnOnce() -> R,
{
    // no-op for async-std
    f()
}

#[cfg(all(feature = "async-native-tls", not(feature = "tokio-native-tls")))]
pub use async_native_tls::{TlsConnector, TlsStream};

#[cfg(all(
    feature = "_tls-rustls",
    feature = "_rt-async-std",
    not(any(
        feature = "_tls-native-tls",
        feature = "_rt-tokio",
        feature = "_rt-actix"
    )),
))]
pub use async_rustls::{client::TlsStream, TlsConnector};
