//! Windows socket options.

use std::os::windows::io::AsRawSocket;
use std::{io, ptr};

use tokio::net::TcpSocket;
use windows_sys::Win32::Networking::WinSock::{
    SIO_TCP_INITIAL_RTO, SOCKET, SOCKET_ERROR, TCP_INITIAL_RTO_PARAMETERS, WSAGetLastError,
    WSAIoctl,
};

/// Windows-only options on a [`TcpSocket`].
pub trait TcpSocketExt {
    /// Fail a connect as soon as the peer refuses it, instead of retransmitting the SYN.
    ///
    /// Windows retransmits by default, so a connect to a port nothing listens on takes about `2s`,
    /// loopback included. Only takes effect before the socket connects.
    fn disable_syn_retransmissions(&self) -> io::Result<()>;
}

impl TcpSocketExt for TcpSocket {
    fn disable_syn_retransmissions(&self) -> io::Result<()> {
        let params = TCP_INITIAL_RTO_PARAMETERS {
            // mstcpip.h's TCP_INITIAL_RTO_DEFAULT_RTT and TCP_INITIAL_RTO_NO_SYN_RETRANSMISSIONS
            // (`(UCHAR) -2`), which windows-sys types wider than these fields.
            Rtt: 0,
            MaxSynRetransmissions: u8::MAX - 1,
        };
        let socket =
            SOCKET::try_from(self.as_raw_socket()).expect("a socket handle is pointer-sized");
        let mut returned = 0;

        // SAFETY: `socket` is open for the call and `params` is the input SIO_TCP_INITIAL_RTO reads.
        // With no output buffer, OVERLAPPED or completion routine, the call is synchronous.
        let status = unsafe {
            WSAIoctl(
                socket,
                SIO_TCP_INITIAL_RTO,
                (&raw const params).cast(),
                u32::try_from(size_of::<TCP_INITIAL_RTO_PARAMETERS>())
                    .expect("TCP_INITIAL_RTO_PARAMETERS is 4 bytes"),
                ptr::null_mut(),
                0,
                &raw mut returned,
                ptr::null_mut(),
                None,
            )
        };
        if status == SOCKET_ERROR {
            // SAFETY: WSAGetLastError has no preconditions.
            return Err(io::Error::from_raw_os_error(unsafe { WSAGetLastError() }));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::net::{Ipv4Addr, SocketAddr};
    use std::time::{Duration, Instant};

    use rstest::{fixture, rstest};

    use super::*;

    /// A loopback address nothing listens on.
    #[fixture]
    fn closed_port() -> SocketAddr {
        std::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap().local_addr().unwrap()
    }

    /// Retransmitting is the control: it proves the check can fail.
    #[rstest]
    #[case::retransmitting(false)]
    #[case::disabled(true)]
    #[tokio::test]
    async fn a_refused_connect_is_slow_only_while_retransmitting(
        closed_port: SocketAddr,
        #[case] disable: bool,
    ) {
        let socket = TcpSocket::new_v4().unwrap();
        if disable {
            socket.disable_syn_retransmissions().unwrap();
        }

        let started = Instant::now();
        let err = socket.connect(closed_port).await.unwrap_err();
        let elapsed = started.elapsed();

        assert_eq!(err.kind(), io::ErrorKind::ConnectionRefused);
        assert_eq!(elapsed >= Duration::from_secs(1), !disable, "took {elapsed:?}");
    }
}
