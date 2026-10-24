//! Extracts and validates images returned by Tools.
//!
//! Tools declare their own MIME types, but declared types are untrusted: a
//! Tool can label arbitrary bytes `image/png`. The model only receives bytes
//! that decode as an image format every supported provider accepts, and the
//! MIME type it receives is the one sniffed from those bytes.

/// One image a Tool returned, held in memory until the Conversation output
/// stores it as a content-addressed Blob.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ToolImage {
    pub(crate) data: Vec<u8>,
    pub(crate) mime_type: String,
    /// Source path when the Tool read a file, otherwise empty.
    pub(crate) path: String,
}

impl ToolImage {
    pub(crate) fn new(data: Vec<u8>, path: impl Into<String>) -> Option<Self> {
        Some(Self {
            mime_type: sniff_mime_type(&data)?.into(),
            data,
            path: path.into(),
        })
    }
}

pub(crate) fn sniff_mime_type(data: &[u8]) -> Option<&'static str> {
    let reader = image::ImageReader::new(std::io::Cursor::new(data))
        .with_guessed_format()
        .ok()?;
    let format = reader.format()?;
    let (width, height) = reader.into_dimensions().ok()?;
    if width == 0 || height == 0 {
        return None;
    }
    match format {
        image::ImageFormat::Png => Some("image/png"),
        image::ImageFormat::Jpeg => Some("image/jpeg"),
        image::ImageFormat::Gif => Some("image/gif"),
        image::ImageFormat::WebP => Some("image/webp"),
        _ => None,
    }
}

#[cfg(test)]
pub(crate) const TEST_PNG: &[u8] = &[
    0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48, 0x44, 0x52,
    0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00, 0x00, 0x1f, 0x15, 0xc4,
    0x89, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x44, 0x41, 0x54, 0x78, 0xda, 0x63, 0xfc, 0xcf, 0xc0, 0x50,
    0x0f, 0x00, 0x04, 0x85, 0x01, 0x80, 0x84, 0xa9, 0x8c, 0x21, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45,
    0x4e, 0x44, 0xae, 0x42, 0x60, 0x82,
];

#[cfg(test)]
pub(crate) const TEST_GIF: &[u8] = &[
    0x47, 0x49, 0x46, 0x38, 0x39, 0x61, 0x01, 0x00, 0x01, 0x00, 0x80, 0x00, 0x00, 0x00, 0x00, 0x00,
    0xff, 0xff, 0xff, 0x21, 0xf9, 0x04, 0x00, 0x00, 0x00, 0x00, 0x00, 0x2c, 0x00, 0x00, 0x00, 0x00,
    0x01, 0x00, 0x01, 0x00, 0x00, 0x02, 0x02, 0x44, 0x01, 0x00, 0x3b,
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepted_formats_are_sniffed_from_bytes() {
        assert_eq!(sniff_mime_type(TEST_PNG), Some("image/png"));
        assert_eq!(sniff_mime_type(TEST_GIF), Some("image/gif"));
    }

    #[test]
    fn undecodable_bytes_are_not_images() {
        assert_eq!(sniff_mime_type(b""), None);
        assert_eq!(sniff_mime_type(b"not an image"), None);
    }

    #[test]
    fn tool_images_derive_their_mime_type_from_bytes() {
        let image = ToolImage::new(TEST_GIF.to_vec(), "capture.gif").unwrap();
        assert_eq!(image.mime_type, "image/gif");
        assert_eq!(image.path, "capture.gif");
        assert!(ToolImage::new(b"not an image".to_vec(), "").is_none());
    }
}
