//! Socket plumbing shared by both roles: length-prefixed frame IO, the
//! per-session inbound rate limit, the close signal and the writer task.

use std::collections::VecDeque;
use std::io;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::sync::{mpsc, Notify};
use tokio::time::Instant;

use crate::proto::{decode_frame, encode_frame, frame_len, Frame, FrameError};

/// TCP connect timeout (ward).
pub(crate) const CONNECT_TIMEOUT: Duration = Duration::from_secs(3);
/// Handshake deadline from connect or accept.
pub(crate) const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(5);
/// The ward's keep-alive interval.
pub(crate) const PING_INTERVAL: Duration = Duration::from_secs(120);
/// A session with no inbound frame for this long is closed.
pub(crate) const IDLE_TIMEOUT: Duration = Duration::from_secs(330);
/// Inbound frames allowed per [`RATE_WINDOW`] before the session closes.
pub(crate) const RATE_MAX: usize = 120;
pub(crate) const RATE_WINDOW: Duration = Duration::from_secs(60);
/// How long a session waits to queue an outbound frame before giving up on
/// a peer that has stopped reading.
pub(crate) const SEND_STALL: Duration = Duration::from_secs(30);

/// Why reading a frame ended the session.
#[derive(Debug)]
pub(crate) enum ReadError {
    Io(io::Error),
    Frame(FrameError),
}

impl std::fmt::Display for ReadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ReadError::Io(e) => write!(f, "{e}"),
            ReadError::Frame(e) => write!(f, "{e}"),
        }
    }
}

/// Read one length-prefixed frame. A zero, oversized or undecodable frame
/// is an error (the caller closes).
pub(crate) async fn read_frame<R: AsyncRead + Unpin>(r: &mut R) -> Result<Frame, ReadError> {
    let mut prefix = [0u8; 4];
    r.read_exact(&mut prefix).await.map_err(ReadError::Io)?;
    let n = frame_len(prefix).map_err(ReadError::Frame)?;
    let mut body = vec![0u8; n];
    r.read_exact(&mut body).await.map_err(ReadError::Io)?;
    decode_frame(&body).map_err(ReadError::Frame)
}

/// Write one frame directly (handshake only; sessions use the writer task).
pub(crate) async fn write_frame<W: AsyncWrite + Unpin>(w: &mut W, frame: &Frame) -> io::Result<()> {
    w.write_all(&encode_frame(frame)).await?;
    w.flush().await
}

/// A fresh 32-byte challenge from the OS.
pub(crate) fn challenge() -> io::Result<[u8; 32]> {
    let mut c = [0u8; 32];
    getrandom::getrandom(&mut c).map_err(|e| io::Error::other(e.to_string()))?;
    Ok(c)
}

/// Sliding-window count of inbound frames.
pub(crate) struct RateLimit {
    seen: VecDeque<Instant>,
}

impl RateLimit {
    pub(crate) fn new() -> Self {
        RateLimit {
            seen: VecDeque::with_capacity(RATE_MAX + 1),
        }
    }

    /// Record one frame; `false` once more than [`RATE_MAX`] frames arrived
    /// within [`RATE_WINDOW`].
    pub(crate) fn hit(&mut self) -> bool {
        let now = Instant::now();
        while let Some(&t) = self.seen.front() {
            if now.duration_since(t) >= RATE_WINDOW {
                self.seen.pop_front();
            } else {
                break;
            }
        }
        self.seen.push_back(now);
        self.seen.len() <= RATE_MAX
    }
}

/// A one-way, level-triggered close signal.
#[derive(Default)]
pub(crate) struct Closer {
    flag: AtomicBool,
    notify: Notify,
}

impl Closer {
    pub(crate) fn close(&self) {
        self.flag.store(true, Ordering::SeqCst);
        self.notify.notify_waiters();
    }

    pub(crate) fn is_closed(&self) -> bool {
        self.flag.load(Ordering::SeqCst)
    }

    pub(crate) async fn closed(&self) {
        loop {
            let n = self.notify.notified();
            tokio::pin!(n);
            n.as_mut().enable();
            if self.is_closed() {
                return;
            }
            n.await;
        }
    }
}

/// Queue a frame on a session's writer; `false` if the session is gone or
/// the peer has stopped reading.
pub(crate) async fn send_frame(tx: &mpsc::Sender<Vec<u8>>, frame: &Frame) -> bool {
    matches!(
        tokio::time::timeout(SEND_STALL, tx.send(encode_frame(frame))).await,
        Ok(Ok(()))
    )
}

/// Drain `rx` into the socket until it closes, a write fails or `closer`
/// fires. A failed write closes the session.
pub(crate) async fn writer<W: AsyncWrite + Unpin>(
    mut w: W,
    mut rx: mpsc::Receiver<Vec<u8>>,
    closer: std::sync::Arc<Closer>,
) {
    loop {
        tokio::select! {
            _ = closer.closed() => break,
            next = rx.recv() => match next {
                Some(buf) => {
                    if w.write_all(&buf).await.is_err() || w.flush().await.is_err() {
                        closer.close();
                        break;
                    }
                }
                None => break,
            },
        }
    }
    let _ = w.shutdown().await;
}
