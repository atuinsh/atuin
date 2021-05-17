use std::io;
use std::mem;
use std::net::SocketAddr;
use std::sync::Once;

use winapi::ctypes::c_int;
use winapi::shared::inaddr::{in_addr_S_un, IN_ADDR};
use winapi::shared::in6addr::{in6_addr_u, IN6_ADDR};
use winapi::shared::ws2def::{AF_INET, AF_INET6, ADDRESS_FAMILY, SOCKADDR, SOCKADDR_IN};
use winapi::shared::ws2ipdef::{SOCKADDR_IN6_LH, SOCKADDR_IN6_LH_u};
use winapi::um::winsock2::{ioctlsocket, socket, FIONBIO, INVALID_SOCKET, SOCKET};

/// Initialise the network stack for Windows.
pub(crate) fn init() {
    static INIT: Once = Once::new();
    INIT.call_once(|| {
        // Let standard library call `WSAStartup` for us, we can't do it
        // ourselves because otherwise using any type in `std::net` would panic
        // when it tries to call `WSAStartup` a second time.
        drop(std::net::UdpSocket::bind("127.0.0.1:0"));
    });
}

/// Create a new non-blocking socket.
pub(crate) fn new_ip_socket(addr: SocketAddr, socket_type: c_int) -> io::Result<SOCKET> {
    use winapi::um::winsock2::{PF_INET, PF_INET6};

    let domain = match addr {
        SocketAddr::V4(..) => PF_INET,
        SocketAddr::V6(..) => PF_INET6,
    };

    new_socket(domain, socket_type)
}

pub(crate) fn new_socket(domain: c_int, socket_type: c_int) -> io::Result<SOCKET> {
    syscall!(
        socket(domain, socket_type, 0),
        PartialEq::eq,
        INVALID_SOCKET
    )
    .and_then(|socket| {
        syscall!(ioctlsocket(socket, FIONBIO, &mut 1), PartialEq::ne, 0).map(|_| socket as SOCKET)
    })
}

/// A type with the same memory layout as `SOCKADDR`. Used in converting Rust level
/// SocketAddr* types into their system representation. The benefit of this specific
/// type over using `SOCKADDR_STORAGE` is that this type is exactly as large as it
/// needs to be and not a lot larger. And it can be initialized cleaner from Rust.
#[repr(C)]
pub(crate) union SocketAddrCRepr {
    v4: SOCKADDR_IN,
    v6: SOCKADDR_IN6_LH,
}

impl SocketAddrCRepr {
    pub(crate) fn as_ptr(&self) -> *const SOCKADDR {
        self as *const _ as *const SOCKADDR
    }
}

pub(crate) fn socket_addr(addr: &SocketAddr) -> (SocketAddrCRepr, c_int) {
    match addr {
        SocketAddr::V4(ref addr) => {
            // `s_addr` is stored as BE on all machine and the array is in BE order.
            // So the native endian conversion method is used so that it's never swapped.
            let sin_addr = unsafe {
                let mut s_un = mem::zeroed::<in_addr_S_un>();
                *s_un.S_addr_mut() = u32::from_ne_bytes(addr.ip().octets());
                IN_ADDR { S_un: s_un }
            };

            let sockaddr_in = SOCKADDR_IN {
                sin_family: AF_INET as ADDRESS_FAMILY,
                sin_port: addr.port().to_be(),
                sin_addr,
                sin_zero: [0; 8],
            };

            let sockaddr = SocketAddrCRepr { v4: sockaddr_in };
            (sockaddr, mem::size_of::<SOCKADDR_IN>() as c_int)
        },
        SocketAddr::V6(ref addr) => {
            let sin6_addr = unsafe {
                let mut u = mem::zeroed::<in6_addr_u>();
                *u.Byte_mut() = addr.ip().octets();
                IN6_ADDR { u }
            };
            let u = unsafe {
                let mut u = mem::zeroed::<SOCKADDR_IN6_LH_u>();
                *u.sin6_scope_id_mut() = addr.scope_id();
                u
            };

            let sockaddr_in6 = SOCKADDR_IN6_LH {
                sin6_family: AF_INET6 as ADDRESS_FAMILY,
                sin6_port: addr.port().to_be(),
                sin6_addr,
                sin6_flowinfo: addr.flowinfo(),
                u,
            };

            let sockaddr = SocketAddrCRepr { v6: sockaddr_in6 };
            (sockaddr, mem::size_of::<SOCKADDR_IN6_LH>() as c_int)
        }
    }
}
