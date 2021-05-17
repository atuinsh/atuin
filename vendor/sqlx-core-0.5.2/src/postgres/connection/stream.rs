use std::ops::{Deref, DerefMut};

use bytes::{Buf, Bytes};
use futures_channel::mpsc::UnboundedSender;
use futures_util::SinkExt;
use log::Level;

use crate::error::Error;
use crate::io::{BufStream, Decode, Encode};
use crate::net::{MaybeTlsStream, Socket};
use crate::postgres::message::{Message, MessageFormat, Notice, Notification};
use crate::postgres::{PgConnectOptions, PgDatabaseError, PgSeverity};

// the stream is a separate type from the connection to uphold the invariant where an instantiated
// [PgConnection] is a **valid** connection to postgres

// when a new connection is asked for, we work directly on the [PgStream] type until the
// connection is fully established

// in other words, `self` in any PgConnection method is a live connection to postgres that
// is fully prepared to receive queries

pub struct PgStream {
    inner: BufStream<MaybeTlsStream<Socket>>,

    // buffer of unreceived notification messages from `PUBLISH`
    // this is set when creating a PgListener and only written to if that listener is
    // re-used for query execution in-between receiving messages
    pub(crate) notifications: Option<UnboundedSender<Notification>>,
}

impl PgStream {
    pub(super) async fn connect(options: &PgConnectOptions) -> Result<Self, Error> {
        let socket = match options.fetch_socket() {
            Some(ref path) => Socket::connect_uds(path).await?,
            None => Socket::connect_tcp(&options.host, options.port).await?,
        };

        let inner = BufStream::new(MaybeTlsStream::Raw(socket));

        Ok(Self {
            inner,
            notifications: None,
        })
    }

    pub(crate) async fn send<'en, T>(&mut self, message: T) -> Result<(), Error>
    where
        T: Encode<'en>,
    {
        self.write(message);
        self.flush().await
    }

    // Expect a specific type and format
    pub(crate) async fn recv_expect<'de, T: Decode<'de>>(
        &mut self,
        format: MessageFormat,
    ) -> Result<T, Error> {
        let message = self.recv().await?;

        if message.format != format {
            return Err(err_protocol!(
                "expecting {:?} but received {:?}",
                format,
                message.format
            ));
        }

        message.decode()
    }

    pub(crate) async fn recv_unchecked(&mut self) -> Result<Message, Error> {
        // all packets in postgres start with a 5-byte header
        // this header contains the message type and the total length of the message
        let mut header: Bytes = self.inner.read(5).await?;

        let format = MessageFormat::try_from_u8(header.get_u8())?;
        let size = (header.get_u32() - 4) as usize;

        let contents = self.inner.read(size).await?;

        Ok(Message { format, contents })
    }

    // Get the next message from the server
    // May wait for more data from the server
    pub(crate) async fn recv(&mut self) -> Result<Message, Error> {
        loop {
            let message = self.recv_unchecked().await?;

            match message.format {
                MessageFormat::ErrorResponse => {
                    // An error returned from the database server.
                    return Err(PgDatabaseError(message.decode()?).into());
                }

                MessageFormat::NotificationResponse => {
                    if let Some(buffer) = &mut self.notifications {
                        let notification: Notification = message.decode()?;
                        let _ = buffer.send(notification).await;

                        continue;
                    }
                }

                MessageFormat::ParameterStatus => {
                    // informs the frontend about the current (initial)
                    // setting of backend parameters

                    // we currently have no use for that data so we promptly ignore this message
                    continue;
                }

                MessageFormat::NoticeResponse => {
                    // do we need this to be more configurable?
                    // if you are reading this comment and think so, open an issue

                    let notice: Notice = message.decode()?;

                    let lvl = match notice.severity() {
                        PgSeverity::Fatal | PgSeverity::Panic | PgSeverity::Error => Level::Error,
                        PgSeverity::Warning => Level::Warn,
                        PgSeverity::Notice => Level::Info,
                        PgSeverity::Debug => Level::Debug,
                        PgSeverity::Info => Level::Trace,
                        PgSeverity::Log => Level::Trace,
                    };

                    if lvl <= log::STATIC_MAX_LEVEL && lvl <= log::max_level() {
                        log::logger().log(
                            &log::Record::builder()
                                .args(format_args!("{}", notice.message()))
                                .level(lvl)
                                .module_path_static(Some("sqlx::postgres::notice"))
                                .file_static(Some(file!()))
                                .line(Some(line!()))
                                .build(),
                        );
                    }

                    continue;
                }

                _ => {}
            }

            return Ok(message);
        }
    }
}

impl Deref for PgStream {
    type Target = BufStream<MaybeTlsStream<Socket>>;

    #[inline]
    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl DerefMut for PgStream {
    #[inline]
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.inner
    }
}
