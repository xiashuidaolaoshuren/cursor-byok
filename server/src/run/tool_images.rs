//! Loads stored Tool images into provider-visible request parts.
use crate::{
    model::{ContentPart, ProjectedContent, ProjectedMessage},
    store::{BlobId, Store},
    Error, Result,
};

/// Materializes every referenced Tool image into the projected history.
///
/// A missing Blob fails the call instead of falling back to text: the
/// provider-visible prefix must not silently lose pixels that an earlier call
/// already saw.
pub(super) async fn hydrate(store: &Store, messages: &mut [ProjectedMessage]) -> Result<()> {
    for message in messages {
        let ProjectedContent::ToolResult(result) = &mut message.content else {
            continue;
        };
        if result.images.is_empty() {
            continue;
        }
        let mut parts = Vec::with_capacity(result.images.len() + 1);
        parts.push(ContentPart::Text {
            text: result.content.clone(),
        });
        for image in &result.images {
            let id = BlobId::from_base64(&image.blob_id)?;
            let data = store.get_blob(&id).await?.ok_or_else(|| {
                Error::Protocol(format!("Tool image Blob is missing: {}", image.blob_id))
            })?;
            parts.push(ContentPart::Image {
                mime_type: image.mime_type.clone(),
                data,
            });
        }
        result.provider_parts = parts;
    }
    Ok(())
}
