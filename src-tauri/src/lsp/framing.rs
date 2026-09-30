//! LSP base-protocol framing: `Content-Length: N\r\n\r\n<N bytes of JSON>`.

use std::io::{self, BufRead};

/// Read one message body. `Ok(None)` means the stream ended cleanly.
pub fn read_message<R: BufRead>(reader: &mut R) -> io::Result<Option<Vec<u8>>> {
    let mut length: Option<usize> = None;
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line)? == 0 {
            return Ok(None);
        }
        let line = line.trim_end();
        if line.is_empty() {
            if length.is_some() {
                break;
            }
            continue; // stray blank line between messages
        }
        if let Some((name, value)) = line.split_once(':') {
            if name.trim().eq_ignore_ascii_case("content-length") {
                length = value.trim().parse().ok();
            }
        }
    }
    let mut body = vec![0u8; length.unwrap_or(0)];
    reader.read_exact(&mut body)?;
    Ok(Some(body))
}

pub fn encode(body: &str) -> Vec<u8> {
    let mut out = format!("Content-Length: {}\r\n\r\n", body.len()).into_bytes();
    out.extend_from_slice(body.as_bytes());
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn round_trips_multiple_messages_with_multibyte_text() {
        let mut wire = encode("{\"a\":\"\u{e9}\"}");
        wire.extend(encode("{\"b\":2}"));
        let mut r = Cursor::new(wire);
        assert_eq!(read_message(&mut r).unwrap().unwrap(), "{\"a\":\"\u{e9}\"}".as_bytes());
        assert_eq!(read_message(&mut r).unwrap().unwrap(), b"{\"b\":2}");
        assert!(read_message(&mut r).unwrap().is_none());
    }

    #[test]
    fn ignores_other_headers() {
        let wire = b"Content-Type: application/vscode-jsonrpc; charset=utf-8\r\ncontent-length: 2\r\n\r\n{}".to_vec();
        assert_eq!(read_message(&mut Cursor::new(wire)).unwrap().unwrap(), b"{}");
    }

    #[test]
    fn truncated_body_is_an_error() {
        let wire = b"Content-Length: 10\r\n\r\n{}".to_vec();
        assert!(read_message(&mut Cursor::new(wire)).is_err());
    }
}
