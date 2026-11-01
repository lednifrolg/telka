//! The socket under a WebSocket: TLS to the TV, or plain TCP.

use std::io;
use std::net::{IpAddr, SocketAddr};
use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::Duration;

use rustls::pki_types::ServerName;
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::net::TcpStream;
use tokio::time::timeout;
use tokio_rustls::TlsConnector;
use tokio_rustls::client::TlsStream;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::{WebSocketStream, client_async};

use crate::error::Error;
use crate::tls::Tls;
use crate::{WS_PORT, WSS_PORT};

/// Where the TV's SSAP API answers, and whether over TLS.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Endpoint {
    pub address: SocketAddr,
    pub secure: bool,
}

impl Endpoint {
    /// The TLS port, [`WSS_PORT`].
    pub fn secure(ip: IpAddr) -> Self {
        Self {
            address: SocketAddr::new(ip, WSS_PORT),
            secure: true,
        }
    }

    /// The plain port, [`WS_PORT`].
    pub fn plain(ip: IpAddr) -> Self {
        Self {
            address: SocketAddr::new(ip, WS_PORT),
            secure: false,
        }
    }

    /// `wss://…` or `ws://…`.
    pub fn url(&self) -> String {
        let scheme = if self.secure { "wss" } else { "ws" };
        format!("{scheme}://{}", self.address)
    }
}

pub(crate) type Socket = WebSocketStream<Connection>;

/// Opens a WebSocket to `url` at `address`, over TLS when `tls` is given. Each step gets `wait`.
pub(crate) async fn open(
    address: SocketAddr,
    url: &str,
    tls: Option<&Tls>,
    wait: Duration,
) -> Result<Socket, Error> {
    let connection = Connection::open(address, tls, wait).await?;
    let request = url.into_client_request().map_err(Error::WebSocket)?;
    let (socket, _) = timeout(wait, client_async(request, connection))
        .await
        .map_err(|_| Error::TimedOut)?
        .map_err(Error::WebSocket)?;
    Ok(socket)
}

/// TLS to the TV, or plain TCP.
pub(crate) enum Connection {
    Plain(TcpStream),
    Tls(Box<TlsStream<TcpStream>>),
}

impl Connection {
    async fn open(address: SocketAddr, tls: Option<&Tls>, wait: Duration) -> Result<Self, Error> {
        let tcp = timeout(wait, TcpStream::connect(address))
            .await
            .map_err(|_| Error::Connect(io::ErrorKind::TimedOut.into()))?
            .map_err(Error::Connect)?;
        let Some(tls) = tls else {
            return Ok(Self::Plain(tcp));
        };
        let name = ServerName::IpAddress(address.ip().into());
        let stream = timeout(
            wait,
            TlsConnector::from(tls.config.clone()).connect(name, tcp),
        )
        .await
        .map_err(|_| Error::Connect(io::ErrorKind::TimedOut.into()))?
        .map_err(|error| match tls.seen() {
            // The verifier only fails on a certificate it does not know.
            Some(found) if is_certificate_error(&error) => Error::UnknownCertificate { found },
            _ => Error::Connect(error),
        })?;
        Ok(Self::Tls(Box::new(stream)))
    }
}

fn is_certificate_error(error: &io::Error) -> bool {
    error
        .get_ref()
        .and_then(|inner| inner.downcast_ref::<rustls::Error>())
        .is_some_and(|tls| matches!(tls, rustls::Error::InvalidCertificate(_)))
}

impl AsyncRead for Connection {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        match self.get_mut() {
            Self::Plain(stream) => Pin::new(stream).poll_read(cx, buf),
            Self::Tls(stream) => Pin::new(stream.as_mut()).poll_read(cx, buf),
        }
    }
}

impl AsyncWrite for Connection {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        match self.get_mut() {
            Self::Plain(stream) => Pin::new(stream).poll_write(cx, buf),
            Self::Tls(stream) => Pin::new(stream.as_mut()).poll_write(cx, buf),
        }
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        match self.get_mut() {
            Self::Plain(stream) => Pin::new(stream).poll_flush(cx),
            Self::Tls(stream) => Pin::new(stream.as_mut()).poll_flush(cx),
        }
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        match self.get_mut() {
            Self::Plain(stream) => Pin::new(stream).poll_shutdown(cx),
            Self::Tls(stream) => Pin::new(stream.as_mut()).poll_shutdown(cx),
        }
    }
}
