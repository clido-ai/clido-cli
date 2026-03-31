//! Shared SSE (Server-Sent Events) byte-stream infrastructure.
//!
//! Both the OpenAI-compatible and Anthropic providers stream responses as SSE.
//! This module extracts the common plumbing: channel setup, chunk timeout,
//! byte-to-line buffering, and line dispatch.

use clido_core::{ClidoError, Result};
use futures::channel::mpsc;
use futures::{SinkExt, StreamExt};

use crate::provider::StreamEvent;

/// Timeout applied to each chunk read from the SSE byte stream.
const STREAM_CHUNK_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(90);

/// Turn a raw byte stream into a `Stream<Item = Result<StreamEvent>>` by:
///
/// 1. Spawning a background task that reads chunks with a 90-second timeout.
/// 2. Buffering bytes into complete lines (split on `\n`).
/// 3. Passing each complete line to `process_line`, which may send zero or
///    more `StreamEvent`s through the provided sender.
///
/// `process_line` receives `(&str, &mut mpsc::UnboundedSender<...>, &mut S)`
/// where `S` is caller-owned state (tool-call accumulators, event buffers, etc.).
/// Return `false` from `process_line` to stop reading.
pub(crate) fn parse_sse_stream<S, F>(
    byte_stream: impl futures::Stream<Item = reqwest::Result<bytes::Bytes>> + Send + 'static,
    mut state: S,
    mut process_line: F,
) -> impl futures::Stream<Item = Result<StreamEvent>> + Send
where
    S: Send + 'static,
    F: FnMut(&str, &mut mpsc::UnboundedSender<Result<StreamEvent>>, &mut S) -> bool
        + Send
        + 'static,
{
    let (mut tx, rx) = mpsc::unbounded::<Result<StreamEvent>>();

    tokio::spawn(async move {
        let mut line_buf = String::new();
        let mut stream = std::pin::pin!(byte_stream);

        loop {
            let chunk = match tokio::time::timeout(STREAM_CHUNK_TIMEOUT, stream.next()).await {
                Ok(Some(chunk)) => chunk,
                Ok(None) => break,
                Err(_elapsed) => {
                    let _ = tx
                        .send(Err(ClidoError::Provider(
                            "streaming stalled — no data received for 90 seconds".to_string(),
                        )))
                        .await;
                    return;
                }
            };
            let bytes = match chunk {
                Ok(b) => b,
                Err(e) => {
                    let _ = tx.send(Err(ClidoError::Provider(e.to_string()))).await;
                    return;
                }
            };

            line_buf.push_str(&String::from_utf8_lossy(&bytes));

            loop {
                let Some(pos) = line_buf.find('\n') else {
                    break;
                };
                let line = line_buf[..pos].trim_end_matches('\r').to_string();
                line_buf = line_buf[pos + 1..].to_string();

                if !process_line(&line, &mut tx, &mut state) {
                    return;
                }
            }
        }
    });

    rx
}
