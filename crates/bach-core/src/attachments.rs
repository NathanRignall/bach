//! Files sent with a message. They travel as `data:` URLs, in the protocol's `images` (which is
//! all they were at first): images as they are, and PDFs and text files with their file name as
//! a parameter, e.g. `data:application/pdf;name=report.pdf;base64,…`.

use base64::{engine::general_purpose::STANDARD as B64, Engine};

/// What Claude accepts as images.
const IMAGE_TYPES: &[&str] = &["image/png", "image/jpeg", "image/gif", "image/webp"];

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Kind {
    Image,
    Pdf,
    /// Any text file (the UI sends them all as `text/plain`).
    Text,
}

/// One attachment's `data:` URL, taken apart.
#[derive(Debug, PartialEq)]
pub struct Attachment<'a> {
    pub mime: &'a str,
    pub kind: Kind,
    /// The file's name, if it came with one (pasted and dropped images often don't).
    pub name: Option<String>,
    /// Base64.
    pub data: &'a str,
}

impl<'a> Attachment<'a> {
    /// `url` as an attachment, if it's one Bach can pass on.
    pub fn parse(url: &'a str) -> Option<Self> {
        let (head, data) = url.strip_prefix("data:")?.split_once(";base64,")?;
        let mut params = head.split(';');
        let mime = params.next()?;
        let kind = match mime {
            m if IMAGE_TYPES.contains(&m) => Kind::Image,
            "application/pdf" => Kind::Pdf,
            "text/plain" => Kind::Text,
            _ => return None,
        };
        let name = params
            .find_map(|p| p.strip_prefix("name="))
            .map(percent_decode)
            .filter(|n| !n.trim().is_empty());
        Some(Self { mime, kind, name, data })
    }

    pub fn bytes(&self) -> Option<Vec<u8>> {
        B64.decode(self.data).ok()
    }

    /// A name to save it under: its own, made safe for a single path component, or a
    /// made-up one.
    pub fn file_name(&self) -> String {
        let own = self.name.as_deref().map(|n| {
            let n: String = n.chars().map(|c| if c == '/' || c == '\\' || c.is_control() { '_' } else { c }).collect();
            n.trim_start_matches('.').chars().take(100).collect::<String>()
        });
        own.filter(|n| !n.is_empty()).unwrap_or_else(|| {
            let ext = match self.mime {
                "image/png" => "png",
                "image/jpeg" => "jpg",
                "image/gif" => "gif",
                "image/webp" => "webp",
                "application/pdf" => "pdf",
                _ => "txt",
            };
            let stem = if self.kind == Kind::Image { "image" } else { "attachment" };
            format!("{stem}.{ext}")
        })
    }
}

/// An attachment saved as a file, for agents that take them by path.
#[derive(Clone, Debug, PartialEq)]
pub struct SavedFile {
    pub path: String,
    pub mime: String,
    pub kind: Kind,
}

/// `%20`-style escapes (the UI's `encodeURIComponent`) undone; bad ones are left as they are.
pub(crate) fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        let hex = b.get(i + 1..i + 3).and_then(|h| u8::from_str_radix(std::str::from_utf8(h).ok()?, 16).ok());
        match (b[i], hex) {
            (b'%', Some(v)) => {
                out.push(v);
                i += 3;
            }
            (c, _) => {
                out.push(c);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_images_pdfs_and_text() {
        let png = Attachment::parse("data:image/png;base64,AAAA").unwrap();
        assert_eq!((png.kind, png.name.as_deref(), png.data), (Kind::Image, None, "AAAA"));
        assert_eq!(png.file_name(), "image.png");

        let pdf = Attachment::parse("data:application/pdf;name=Q3%20report.pdf;base64,AAAA").unwrap();
        assert_eq!((pdf.kind, pdf.mime, pdf.name.as_deref()), (Kind::Pdf, "application/pdf", Some("Q3 report.pdf")));
        assert_eq!(pdf.file_name(), "Q3 report.pdf");

        let text = Attachment::parse("data:text/plain;name=..%2F..%2Fetc%2Fpasswd;base64,aGk=").unwrap();
        assert_eq!(text.kind, Kind::Text);
        assert_eq!(text.file_name(), "_.._etc_passwd");
        assert_eq!(text.bytes().unwrap(), b"hi");

        assert_eq!(Attachment::parse("data:text/plain;base64,aGk=").unwrap().file_name(), "attachment.txt");
        for bad in ["data:application/zip;base64,AAAA", "data:image/png,AAAA", "https://x/a.png", "data:image/svg+xml;base64,AAAA"] {
            assert_eq!(Attachment::parse(bad), None, "{bad}");
        }
    }

    #[test]
    fn percent_decoding() {
        assert_eq!(percent_decode("a%20b%E2%9C%93%zz%4"), "a b✓%zz%4");
    }
}
