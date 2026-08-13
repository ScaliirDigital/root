//! What travels between the server and a render worker.
//!
//! The frame format lives here so the async server side and blocking worker
//! side share the same framing rules.
//!
//! Format: length-prefixed frames, u32 little-endian length followed by that
//! many bytes. EOF cannot delimit a message because the worker stays alive for
//! the next job.
//!
//! A request is one frame: the encoded [`Job`]. A response is two: a
//! [`JobResult`] as JSON, then the raw PDF. The PDF is deliberately kept out of
//! JSON because encoding binary data as an array of numbers would inflate it
//! substantially.

use std::io;

use serde::{Deserialize, Serialize};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

use crate::core::{engine::RenderOptions, files::Files};

/// Upper bound on a single frame. Guards against a corrupted length header
/// turning into a multi-gigabyte allocation.
pub const MAX_FRAME_BYTES: usize = 64 * 1024 * 1024;

/// One unit of work, sent to a worker.
#[derive(Serialize, Deserialize)]
pub struct Job {
    pub files: Files,

    /// `None` means no input is passed to the compiler at all -- not an empty
    /// object. A template's own fallback only runs when `sys.inputs` is unset.
    pub data: Option<serde_json::Value>,

    /// Machine-readable data to attach to the PDF, for standards that pair a
    /// human-readable document with a structured mirror of it -- `ZUGFeRD` and
    /// Factur-X for invoices. Passed through untouched: whether it conforms to
    /// EN 16931 is the caller's business, not ours.
    #[serde(default)]
    pub xml: Option<String>,

    #[serde(with = "options_repr")]
    pub options: RenderOptions,
}

/// The metadata frame of a response. The PDF follows as its own raw frame.
#[derive(Serialize, Deserialize)]
pub enum JobResult {
    Ok {
        warnings: Vec<String>,
    },
    Failed {
        errors: Vec<String>,
        warnings: Vec<String>,
    },
}

/// The four-byte length prefix that introduces a frame.
///
/// The size limit is enforced in one direction only, and deliberately so: a
/// length that arrives over the wire is untrusted and could turn into a huge
/// allocation, while a length we compute from a buffer we already hold cannot
/// surprise us. A writer that still manages to produce an oversized frame is
/// caught by the reader on the other end.
pub struct Frame([u8; 4]);

impl Frame {
    /// Encodes a payload length as a wire header.
    #[must_use]
    pub fn encode(length: usize) -> Self {
        let length = u32::try_from(length).expect("payload length fits in u32");

        Self(length.to_le_bytes())
    }

    /// Decodes a wire header into the payload length that follows it.
    ///
    /// # Errors
    ///
    /// If the header announces more than [`MAX_FRAME_BYTES`].
    pub fn decode(bytes: [u8; 4]) -> io::Result<usize> {
        let length = u32::from_le_bytes(bytes) as usize;

        if length > MAX_FRAME_BYTES {
            return Err(io::Error::other("frame exceeds maximum size"));
        }

        Ok(length)
    }

    #[must_use]
    pub fn as_bytes(&self) -> &[u8; 4] {
        &self.0
    }
}

// ---------------------------------------------------------------------------
// Server side: async
// ---------------------------------------------------------------------------

pub async fn write_frame(
    sink: &mut (dyn AsyncWrite + Unpin + Send),
    payload: &[u8],
) -> io::Result<()> {
    let header = Frame::encode(payload.len());

    sink.write_all(header.as_bytes()).await?;
    sink.write_all(payload).await?;
    sink.flush().await
}

pub async fn read_frame(source: &mut (dyn AsyncRead + Unpin + Send)) -> io::Result<Vec<u8>> {
    let mut header = [0u8; 4];
    source.read_exact(&mut header).await?;

    let length = Frame::decode(header)?;
    let mut payload = vec![0u8; length];

    source.read_exact(&mut payload).await?;
    Ok(payload)
}

// ---------------------------------------------------------------------------
// Worker side: blocking
// ---------------------------------------------------------------------------

/// Reads one frame, or `None` on EOF -- which is how the parent says
/// "no more work, shut down".
pub fn read_frame_blocking(source: &mut dyn io::Read) -> io::Result<Option<Vec<u8>>> {
    let mut header = [0u8; 4];

    match source.read_exact(&mut header) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::UnexpectedEof => {
            return Ok(None);
        }
        Err(error) => return Err(error),
    }

    let length = Frame::decode(header)?;
    let mut payload = vec![0u8; length];

    source.read_exact(&mut payload)?;
    Ok(Some(payload))
}

pub fn write_frame_blocking(sink: &mut dyn io::Write, payload: &[u8]) -> io::Result<()> {
    let header = Frame::encode(payload.len());

    sink.write_all(header.as_bytes())?;
    sink.write_all(payload)?;
    sink.flush()
}

// ---------------------------------------------------------------------------

/// `RenderOptions` is not Serialize by itself; the wire shape lives here so the
/// engine stays free of transport concerns.
mod options_repr {
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    use crate::core::engine::{Pdf, RenderOptions};

    #[derive(Serialize, Deserialize)]
    struct Repr {
        timestamp: Option<i64>,
        a3b: bool,
    }

    pub fn serialize<S: Serializer>(
        value: &RenderOptions,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        Repr {
            timestamp: value.timestamp,
            a3b: value.standard == Pdf::A3b,
        }
        .serialize(serializer)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<RenderOptions, D::Error> {
        let repr = Repr::deserialize(deserializer)?;

        Ok(RenderOptions {
            timestamp: repr.timestamp,
            standard: if repr.a3b { Pdf::A3b } else { Pdf::Plain },
        })
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn deserializes_plain_pdf_options() {
            let mut deserializer =
                serde_json::Deserializer::from_str(r#"{"timestamp":null,"a3b":false}"#);

            let options = deserialize(&mut deserializer).expect("deserialize render options");

            assert_eq!(options.timestamp, None);
            assert_eq!(options.standard, Pdf::Plain);
        }

        /// The archival path is the one that matters legally, so its wire shape
        /// gets its own test rather than riding along on an end-to-end render.
        #[test]
        fn deserializes_archival_pdf_options() {
            let mut deserializer =
                serde_json::Deserializer::from_str(r#"{"timestamp":0,"a3b":true}"#);

            let options = deserialize(&mut deserializer).expect("deserialize render options");

            assert_eq!(options.timestamp, Some(0));
            assert_eq!(options.standard, Pdf::A3b);
        }

        #[test]
        fn rejects_invalid_render_options() {
            let mut deserializer =
                serde_json::Deserializer::from_str(r#"{"timestamp":null,"a3b":"invalid"}"#);

            assert!(deserialize(&mut deserializer).is_err());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::{
        pin::Pin,
        task::{Context, Poll},
    };

    use tokio::io::{AsyncWrite, AsyncWriteExt};

    struct FailingWriter {
        written: usize,
        fail_after: usize,
    }

    impl FailingWriter {
        fn new(fail_after: usize) -> Self {
            Self {
                written: 0,
                fail_after,
            }
        }
    }

    impl io::Write for FailingWriter {
        fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
            if self.written >= self.fail_after {
                return Err(io::Error::other("write failed"));
            }

            let remaining = self.fail_after - self.written;
            let written = buffer.len().min(remaining);

            self.written += written;
            Ok(written)
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    struct FailingAsyncWriter {
        written: usize,
        fail_after: usize,
    }

    impl FailingAsyncWriter {
        fn new(fail_after: usize) -> Self {
            Self {
                written: 0,
                fail_after,
            }
        }
    }

    impl AsyncWrite for FailingAsyncWriter {
        fn poll_write(
            mut self: Pin<&mut Self>,
            _cx: &mut Context<'_>,
            buffer: &[u8],
        ) -> Poll<io::Result<usize>> {
            if self.written >= self.fail_after {
                return Poll::Ready(Err(io::Error::other("write failed")));
            }

            let remaining = self.fail_after - self.written;
            let written = buffer.len().min(remaining);

            self.written += written;
            Poll::Ready(Ok(written))
        }

        fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
            Poll::Ready(Ok(()))
        }

        fn poll_shutdown(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
            Poll::Ready(Ok(()))
        }
    }

    struct FailingReader;

    impl io::Read for FailingReader {
        fn read(&mut self, _buffer: &mut [u8]) -> io::Result<usize> {
            Err(io::Error::other("read failed"))
        }
    }

    // -----------------------------------------------------------------------
    // Frame
    // -----------------------------------------------------------------------

    #[test]
    fn frame_encodes_length() {
        assert_eq!(u32::from_le_bytes(*Frame::encode(7).as_bytes()), 7);
    }

    #[test]
    fn frame_encodes_maximum_size() {
        assert_eq!(
            u32::from_le_bytes(*Frame::encode(MAX_FRAME_BYTES).as_bytes()),
            u32::try_from(MAX_FRAME_BYTES).expect("maximum frame size fits in u32")
        );
    }

    #[test]
    fn frame_decodes_length() {
        assert_eq!(Frame::decode(7_u32.to_le_bytes()).expect("valid header"), 7);
    }

    #[test]
    fn frame_decodes_maximum_size() {
        let length = u32::try_from(MAX_FRAME_BYTES).expect("maximum frame fits in u32");

        assert_eq!(
            Frame::decode(length.to_le_bytes()).expect("maximum frame length"),
            MAX_FRAME_BYTES
        );
    }

    #[test]
    fn frame_rejects_oversized_length() {
        let length = u32::try_from(MAX_FRAME_BYTES + 1).expect("oversized test frame fits in u32");

        let result = Frame::decode(length.to_le_bytes());

        assert_eq!(
            result.expect_err("oversized frame must fail").kind(),
            io::ErrorKind::Other
        );
    }

    // -----------------------------------------------------------------------
    // Async/blocking compatibility
    // -----------------------------------------------------------------------

    /// The two implementations must agree or every render fails with a desync.
    #[tokio::test]
    async fn blocking_writer_and_async_reader_agree() {
        let payload = b"%PDF-1.7 fake".to_vec();

        let mut wire = Vec::new();
        write_frame_blocking(&mut wire, &payload).unwrap();

        let mut cursor = wire.as_slice();

        assert_eq!(read_frame(&mut cursor).await.unwrap(), payload);
    }

    #[tokio::test]
    async fn async_writer_and_blocking_reader_agree() {
        let payload = b"%PDF-1.7 fake".to_vec();

        let mut wire = Vec::new();
        write_frame(&mut wire, &payload).await.unwrap();

        let mut cursor = wire.as_slice();

        assert_eq!(
            read_frame_blocking(&mut cursor).unwrap(),
            Some(payload.clone())
        );

        // Nothing left: EOF is the shutdown signal.
        assert!(read_frame_blocking(&mut cursor).unwrap().is_none());
    }

    // -----------------------------------------------------------------------
    // Readers
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn async_reader_rejects_oversized_frame() {
        let length = u32::try_from(MAX_FRAME_BYTES + 1).expect("frame length fits in u32");

        let mut input = std::io::Cursor::new(length.to_le_bytes());

        let result = read_frame(&mut input).await;

        assert_eq!(
            result.expect_err("oversized frame must fail").kind(),
            io::ErrorKind::Other
        );
    }

    #[test]
    fn blocking_reader_rejects_oversized_frame() {
        let length = u32::try_from(MAX_FRAME_BYTES + 1).expect("frame length fits in u32");

        let mut input = std::io::Cursor::new(length.to_le_bytes());

        let result = read_frame_blocking(&mut input);

        assert_eq!(
            result.expect_err("oversized frame must fail").kind(),
            io::ErrorKind::Other
        );
    }

    #[tokio::test]
    async fn async_reader_rejects_truncated_header() {
        let mut source = &[1_u8, 2][..];

        let result = read_frame(&mut source).await;

        assert_eq!(
            result.expect_err("truncated header must fail").kind(),
            io::ErrorKind::UnexpectedEof
        );
    }

    #[tokio::test]
    async fn async_reader_rejects_truncated_payload() {
        let mut wire = 4_u32.to_le_bytes().to_vec();
        wire.extend_from_slice(b"ab");

        let mut source = wire.as_slice();

        let result = read_frame(&mut source).await;

        assert_eq!(
            result.expect_err("truncated payload must fail").kind(),
            io::ErrorKind::UnexpectedEof
        );
    }

    #[test]
    fn blocking_reader_returns_none_on_eof() {
        let mut source = &[][..];

        assert!(
            read_frame_blocking(&mut source)
                .expect("clean EOF")
                .is_none()
        );
    }

    #[test]
    fn blocking_reader_returns_none_on_truncated_header() {
        let mut source = &[1_u8, 2][..];

        assert!(
            read_frame_blocking(&mut source)
                .expect("truncated header")
                .is_none()
        );
    }

    #[test]
    fn blocking_reader_rejects_truncated_payload() {
        let mut wire = 4_u32.to_le_bytes().to_vec();
        wire.extend_from_slice(b"ab");

        let mut source = wire.as_slice();

        let result = read_frame_blocking(&mut source);

        assert_eq!(
            result.expect_err("truncated payload must fail").kind(),
            io::ErrorKind::UnexpectedEof
        );
    }

    #[test]
    fn blocking_reader_propagates_read_error() {
        let result = read_frame_blocking(&mut FailingReader);

        assert_eq!(
            result.expect_err("read error must propagate").kind(),
            io::ErrorKind::Other
        );
    }

    // -----------------------------------------------------------------------
    // Writers
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn async_writer_propagates_header_error() {
        let mut sink = FailingAsyncWriter::new(0);

        let result = write_frame(&mut sink, b"payload").await;

        assert!(result.is_err());
    }

    #[tokio::test]
    async fn async_writer_propagates_payload_error() {
        // Four bytes allow the header through and fail on the payload.
        let mut sink = FailingAsyncWriter::new(4);

        let result = write_frame(&mut sink, b"payload").await;

        assert!(result.is_err());
    }

    #[test]
    fn blocking_writer_propagates_header_error() {
        let mut sink = FailingWriter::new(0);

        let result = write_frame_blocking(&mut sink, b"payload");

        assert!(result.is_err());
    }

    #[test]
    fn blocking_writer_propagates_payload_error() {
        // Four bytes allow the header through and fail on the payload.
        let mut sink = FailingWriter::new(4);

        let result = write_frame_blocking(&mut sink, b"payload");

        assert!(result.is_err());
    }

    // Exercise successful flush/shutdown behavior of the fault writers so the
    // test doubles themselves remain covered.

    #[test]
    fn blocking_fault_writer_can_succeed() {
        let mut sink = FailingWriter::new(usize::MAX);

        write_frame_blocking(&mut sink, b"payload").expect("write frame");
    }

    #[tokio::test]
    async fn async_fault_writer_can_succeed() {
        let mut sink = FailingAsyncWriter::new(usize::MAX);

        write_frame(&mut sink, b"payload")
            .await
            .expect("write frame");
    }

    #[tokio::test]
    async fn async_fault_writer_can_shutdown() {
        let mut sink = FailingAsyncWriter::new(usize::MAX);

        sink.shutdown().await.expect("shutdown");
    }
}
