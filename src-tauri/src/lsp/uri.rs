//! Minimal `file://` URI conversion (no dependency needed for this).

use std::path::{Path, PathBuf};

fn is_unreserved(b: u8) -> bool {
    b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~' | b'/' | b':')
}

/// `C:\Users\me\a b.cpp` -> `file:///C:/Users/me/a%20b.cpp`
pub fn path_to_uri(path: &Path) -> String {
    let s = path.to_string_lossy().replace('\\', "/");
    let mut out = String::from("file://");
    if !s.starts_with('/') {
        out.push('/');
    }
    for b in s.bytes() {
        if is_unreserved(b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

fn hex(b: u8) -> Option<u8> {
    (b as char).to_digit(16).map(|d| d as u8)
}

fn percent_decode(s: &str) -> Vec<u8> {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let (Some(h), Some(l)) = (hex(bytes[i + 1]), hex(bytes[i + 2])) {
                out.push(h * 16 + l);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    out
}

/// `file:///c%3A/x/y.h` -> `c:\x\y.h` (Windows) / `/x/y.h` (elsewhere). `None` if not a file URI.
pub fn uri_to_path(uri: &str) -> Option<PathBuf> {
    let rest = uri.strip_prefix("file://")?;
    // Drop an authority (`file://host/...`); local URIs have an empty one.
    let rest = &rest[rest.find('/')?..];
    let decoded = String::from_utf8(percent_decode(rest)).ok()?;
    if cfg!(windows) {
        let p = decoded.strip_prefix('/').unwrap_or(&decoded);
        Some(PathBuf::from(p.replace('/', "\\")))
    } else {
        Some(PathBuf::from(decoded))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encodes_spaces_and_unicode() {
        assert_eq!(path_to_uri(Path::new("C:\\Users\\a b\\\u{e9}.cpp")), "file:///C:/Users/a%20b/%C3%A9.cpp");
    }

    #[cfg(windows)]
    #[test]
    fn decodes_clangd_style_uris() {
        assert_eq!(uri_to_path("file:///c%3A/Program%20Files/x.h").unwrap(), PathBuf::from(r"c:\Program Files\x.h"));
        assert!(uri_to_path("https://example.com/x").is_none());
    }

    #[test]
    fn decode_handles_trailing_percent() {
        assert_eq!(percent_decode("a%"), b"a%");
        assert_eq!(percent_decode("a%4"), b"a%4");
        assert_eq!(percent_decode("%41"), b"A");
    }
}
