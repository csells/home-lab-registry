use std::{
    future::{Future, Ready, ready},
    io,
    pin::Pin,
    task::{Context, Poll},
    time::Duration,
};

use axum_server::{
    Server,
    accept::Accept,
    tls_rustls::{RustlsAcceptor, RustlsConfig},
};
use hyper_util::rt::TokioTimer;
use tokio::{
    io::{AsyncRead, AsyncWrite, ReadBuf},
    net::TcpStream,
    time::{Sleep, sleep},
};

pub fn tls_with_deadlines(
    server: Server<std::net::SocketAddr>,
    tls: RustlsConfig,
    deadline: Duration,
) -> Server<std::net::SocketAddr, RustlsAcceptor<WriteDeadline>> {
    let mut server = server
        .acceptor(RustlsAcceptor::new(tls).acceptor(WriteDeadline(deadline)))
        .http1_only();
    server
        .http_builder()
        .http1()
        .timer(TokioTimer::new())
        .header_read_timeout(deadline);
    server
}

#[derive(Clone)]
pub struct WriteDeadline(Duration);

impl<S> Accept<TcpStream, S> for WriteDeadline {
    type Stream = DeadlineStream<TcpStream>;
    type Service = S;
    type Future = Ready<io::Result<(Self::Stream, S)>>;

    fn accept(&self, stream: TcpStream, service: S) -> Self::Future {
        ready(Ok((with_write_deadline(stream, self.0), service)))
    }
}

pub struct DeadlineStream<T> {
    inner: T,
    duration: Duration,
    stalled: Option<Pin<Box<Sleep>>>,
}

fn with_write_deadline<T>(inner: T, duration: Duration) -> DeadlineStream<T> {
    DeadlineStream {
        inner,
        duration,
        stalled: None,
    }
}

impl<T> DeadlineStream<T> {
    fn write_progress<R>(
        &mut self,
        cx: &mut Context<'_>,
        result: Poll<io::Result<R>>,
    ) -> Poll<io::Result<R>> {
        if result.is_ready() {
            self.stalled = None;
            return result;
        }
        // No timer is running when the response body has no bytes ready.
        let stalled = self
            .stalled
            .get_or_insert_with(|| Box::pin(sleep(self.duration)));
        if stalled.as_mut().poll(cx).is_ready() {
            return Poll::Ready(Err(io::ErrorKind::TimedOut.into()));
        }
        Poll::Pending
    }
}

impl<T: AsyncRead + Unpin> AsyncRead for DeadlineStream<T> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        Pin::new(&mut self.inner).poll_read(cx, buf)
    }
}

impl<T: AsyncWrite + Unpin> AsyncWrite for DeadlineStream<T> {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        data: &[u8],
    ) -> Poll<io::Result<usize>> {
        let result = Pin::new(&mut self.inner).poll_write(cx, data);
        self.write_progress(cx, result)
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let result = Pin::new(&mut self.inner).poll_flush(cx);
        self.write_progress(cx, result)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let result = Pin::new(&mut self.inner).poll_shutdown(cx);
        self.write_progress(cx, result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::ErrorKind;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    #[tokio::test(start_paused = true)]
    async fn a_blocked_write_has_its_own_deadline() {
        let (socket, _peer) = tokio::io::duplex(1);
        let mut io = with_write_deadline(socket, Duration::from_secs(8));
        let result = tokio::time::timeout(Duration::from_secs(9), io.write_all(b"two")).await;
        assert_eq!(
            result.expect("transport deadline").unwrap_err().kind(),
            ErrorKind::TimedOut
        );
    }

    #[tokio::test(start_paused = true)]
    async fn idle_time_and_progress_do_not_consume_a_write_budget() {
        let (socket, mut peer) = tokio::io::duplex(1);
        let mut io = with_write_deadline(socket, Duration::from_secs(8));
        tokio::time::sleep(Duration::from_secs(30)).await;
        let send = async { io.write_all(b"data").await.unwrap() };
        let receive = async {
            for expected in b"data" {
                tokio::time::sleep(Duration::from_secs(4)).await;
                assert_eq!(peer.read_u8().await.unwrap(), *expected);
            }
        };
        tokio::join!(send, receive);
    }
}
