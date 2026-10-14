//! Content-Length framed DAP wire codec.
//!
//! DAP's base protocol frames every message as `Content-Length: <n>`
//! headers, a blank line, then an `n`-byte body. This codec is the single
//! framing implementation for the proxy; the test harness and the scripted
//! fake adapter each carry an intentionally separate copy so a framing bug
//! cannot cancel itself out across the wire boundary.

use std::fmt;
use std::io::{self, BufRead, Write};

/// Header blocks past this size are not DAP — the stream is garbage.
pub const MAX_HEADER_BYTES: usize = 8 * 1024;

/// Largest body a peer may declare. Content-Length arrives on an untrusted
/// stream, and honouring an arbitrary declaration would size an allocation
/// on the sender's word — an allocator abort on garbage is a remote kill of
/// the pipe. Past the bound the declaration itself is the protocol error.
pub const MAX_BODY_BYTES: usize = 64 * 1024 * 1024;

#[derive(Debug)]
pub enum FrameError {
    Io(io::Error),
    /// The byte stream broke the base protocol; the frame boundary cannot be
    /// trusted, so the stream cannot be re-synchronised.
    Protocol(String),
}

impl fmt::Display for FrameError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            FrameError::Io(e) => write!(f, "io error: {e}"),
            FrameError::Protocol(why) => write!(f, "protocol error: {why}"),
        }
    }
}

impl From<io::Error> for FrameError {
    fn from(e: io::Error) -> Self {
        FrameError::Io(e)
    }
}

/// Read one frame body. `Ok(None)` only for a clean EOF before any header
/// byte — the session simply ended; anything torn mid-frame is a protocol
/// break the caller cannot resynchronise.
pub fn read_frame(reader: &mut impl BufRead) -> Result<Option<Vec<u8>>, FrameError> {
    let mut header: Vec<u8> = Vec::new();
    let mut byte = [0u8; 1];
    loop {
        let n = reader.read(&mut byte)?;
        if n == 0 {
            if header.is_empty() {
                return Ok(None);
            }
            return Err(FrameError::Protocol("stream ended inside the header block".into()));
        }
        header.push(byte[0]);
        // Both delimiter spellings are accepted; some client stacks emit bare LFs.
        if header.ends_with(b"\r\n\r\n") || header.ends_with(b"\n\n") {
            break;
        }
        if header.len() > MAX_HEADER_BYTES {
            return Err(FrameError::Protocol("header block exceeds the limit".into()));
        }
    }
    let mut length: Option<usize> = None;
    for line in String::from_utf8_lossy(&header).split('\n') {
        if let Some((key, value)) = line.trim().split_once(':') {
            if key.trim().eq_ignore_ascii_case("content-length") {
                length = value.trim().parse().ok();
            }
        }
    }
    let length = length
        .ok_or_else(|| FrameError::Protocol("header block carries no usable Content-Length".into()))?;
    if length > MAX_BODY_BYTES {
        return Err(FrameError::Protocol("declared body exceeds the size limit".into()));
    }
    let mut body = vec![0u8; length];
    reader
        .read_exact(&mut body)
        .map_err(|_| FrameError::Protocol("stream ended inside the frame body".into()))?;
    Ok(Some(body))
}

/// Frame and deliver one message body. Every call flushes: each DAP message
/// is a unit of progress, and a relay that holds one back shows the client a
/// hang instead of a session.
pub fn write_frame(writer: &mut impl Write, body: &[u8]) -> io::Result<()> {
    let header = format!("Content-Length: {}\r\n\r\n", body.len());
    writer.write_all(header.as_bytes())?;
    writer.write_all(body)?;
    writer.flush()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn framed(body: &str) -> String {
        format!("Content-Length: {}\r\n\r\n{}", body.len(), body)
    }

    #[test]
    fn reads_a_crlf_delimited_frame() {
        let mut cursor = Cursor::new(framed(r#"{"seq":1}"#));
        assert_eq!(read_frame(&mut cursor).unwrap(), Some(r#"{"seq":1}"#.as_bytes().to_vec()));
    }

    #[test]
    fn accepts_bare_lf_delimiters_and_a_lowercase_header() {
        let mut cursor = Cursor::new("content-length: 2\n\n{}".to_string());
        assert_eq!(read_frame(&mut cursor).unwrap(), Some(b"{}".to_vec()));
    }

    #[test]
    fn missing_content_length_is_a_protocol_error() {
        let mut cursor = Cursor::new("X-Whatever: 1\r\n\r\n{}".to_string());
        assert!(matches!(read_frame(&mut cursor), Err(FrameError::Protocol(_))));
    }

    #[test]
    fn clean_eof_before_a_frame_reads_as_none() {
        let mut cursor = Cursor::new(Vec::<u8>::new());
        assert_eq!(read_frame(&mut cursor).unwrap(), None);
    }

    #[test]
    fn truncated_body_is_a_protocol_error() {
        let mut cursor = Cursor::new("Content-Length: 99\r\n\r\nshort".to_string());
        assert!(matches!(read_frame(&mut cursor), Err(FrameError::Protocol(_))));
    }

    #[test]
    fn oversized_header_is_a_protocol_error() {
        let mut cursor = Cursor::new(vec![b'x'; MAX_HEADER_BYTES + 1]);
        assert!(matches!(read_frame(&mut cursor), Err(FrameError::Protocol(_))));
    }

    #[test]
    fn multibyte_bodies_are_counted_in_bytes() {
        // Content-Length counts bytes, never chars — write and read must agree
        // on non-ASCII payloads or the stream desynchronises.
        let body = "äöü\n".as_bytes().to_vec();
        let mut buffer = Vec::new();
        write_frame(&mut buffer, &body).unwrap();
        let mut cursor = Cursor::new(buffer);
        assert_eq!(read_frame(&mut cursor).unwrap(), Some(body));
    }

    #[test]
    fn a_huge_declared_body_is_refused_before_allocating() {
        // Content-Length is untrusted input: a declaration at the usize
        // ceiling must be refused from the header alone. Uncapped, this
        // reaches vec![0u8; length] and panics the process (capacity
        // overflow) — a garbage sender killing the pipe by declaration.
        let header = format!("Content-Length: {}\r\n\r\n", usize::MAX);
        let mut cursor = Cursor::new(header);
        assert!(matches!(read_frame(&mut cursor), Err(FrameError::Protocol(_))));
    }

    #[test]
    fn consecutive_frames_come_back_in_order() {
        let mut buffer = Vec::new();
        write_frame(&mut buffer, b"a").unwrap();
        write_frame(&mut buffer, b"bb").unwrap();
        let mut cursor = Cursor::new(buffer);
        assert_eq!(read_frame(&mut cursor).unwrap(), Some(b"a".to_vec()));
        assert_eq!(read_frame(&mut cursor).unwrap(), Some(b"bb".to_vec()));
        assert_eq!(read_frame(&mut cursor).unwrap(), None);
    }
}
