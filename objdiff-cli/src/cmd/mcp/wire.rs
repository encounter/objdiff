//! Wire-level logging of MCP traffic.
//!
//! Everything here sits *below* the rmcp handler: each JSON-RPC message is
//! logged exactly as it appears on the transport, in both directions, so a
//! session log shows the raw client/server conversation (including
//! notifications and messages rmcp answers by itself).

use std::{
    io,
    pin::Pin,
    task::{Context, Poll},
};

use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};

/// Prefix for messages received from the client.
pub const RECV: &str = "mcp <-";
/// Prefix for messages sent to the client.
pub const SEND: &str = "mcp ->";

/// Characters of a message logged at info level. A single `diff_function`
/// response runs to tens of kilobytes, so info gets a preview and debug gets
/// the whole thing.
const PREVIEW_CHARS: usize = 800;

/// An SSE frame made only of comment lines (`:`), i.e. a keep-alive ping. The
/// http transport sends one every 15 seconds; they carry no protocol data.
fn is_sse_keep_alive(text: &str) -> bool {
    text.lines().all(|line| line.trim_end().is_empty() || line.starts_with(':'))
}

/// Log one raw message (a JSON-RPC line, or an HTTP/SSE body chunk).
pub fn log_payload(direction: &str, bytes: &[u8]) {
    let text = String::from_utf8_lossy(bytes);
    let text = text.trim();
    if text.is_empty() {
        return;
    }
    if is_sse_keep_alive(text) {
        tracing::trace!("{direction} keep-alive");
        return;
    }
    match text.char_indices().nth(PREVIEW_CHARS) {
        Some((idx, _)) => {
            tracing::info!("{direction} {}… [{} bytes]", &text[..idx], text.len());
            tracing::debug!("{direction} (full) {text}");
        }
        None => tracing::info!("{direction} {text}"),
    }
}

/// Accumulates transport bytes and logs each complete newline-delimited message.
struct LineLogger {
    direction: &'static str,
    buf: Vec<u8>,
}

impl LineLogger {
    fn new(direction: &'static str) -> Self { Self { direction, buf: Vec::new() } }

    fn push(&mut self, bytes: &[u8]) {
        self.buf.extend_from_slice(bytes);
        while let Some(pos) = self.buf.iter().position(|&b| b == b'\n') {
            let line: Vec<u8> = self.buf.drain(..=pos).collect();
            log_payload(self.direction, &line);
        }
    }
}

/// Reader wrapper that logs every message received from the client.
pub struct LoggingRead<R> {
    inner: R,
    logger: LineLogger,
}

impl<R> LoggingRead<R> {
    pub fn new(inner: R) -> Self { Self { inner, logger: LineLogger::new(RECV) } }
}

impl<R: AsyncRead + Unpin> AsyncRead for LoggingRead<R> {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        let start = buf.filled().len();
        let poll = Pin::new(&mut this.inner).poll_read(cx, buf);
        if matches!(poll, Poll::Ready(Ok(()))) {
            let new = &buf.filled()[start..];
            if !new.is_empty() {
                let new = new.to_vec();
                this.logger.push(&new);
            }
        }
        poll
    }
}

/// Writer wrapper that logs every message sent to the client.
pub struct LoggingWrite<W> {
    inner: W,
    logger: LineLogger,
}

impl<W> LoggingWrite<W> {
    pub fn new(inner: W) -> Self { Self { inner, logger: LineLogger::new(SEND) } }
}

impl<W: AsyncWrite + Unpin> AsyncWrite for LoggingWrite<W> {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        let this = self.get_mut();
        let poll = Pin::new(&mut this.inner).poll_write(cx, buf);
        // Only log what the inner writer actually accepted.
        if let Poll::Ready(Ok(n)) = &poll {
            this.logger.push(&buf[..*n]);
        }
        poll
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        Pin::new(&mut this.inner).poll_flush(cx)
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        Pin::new(&mut this.inner).poll_shutdown(cx)
    }
}
