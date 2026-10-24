//! Restores Conversation Messages and pending Tool state from a checkpoint.
use crate::{
    cursor::{checkpoint::messages, protocol::proto::agent::v1 as pb},
    model::{CanonicalMessage, MessageContent},
    store::BlobId,
    Error, Result,
};

use super::CheckpointBuilder;

impl CheckpointBuilder {
    pub async fn import_prefetched(&self, blobs: &[pb::PreFetchedBlob]) -> Result<()> {
        for blob in blobs {
            let expected = BlobId::from_bytes(&blob.id)?;
            let actual = self.store.put_blob(&blob.value, &[]).await?;
            if expected != actual {
                return Err(Error::Protocol(format!(
                    "prefetched Blob hash mismatch: {}",
                    expected.to_base64()
                )));
            }
        }
        Ok(())
    }

    pub async fn hydrate_messages(
        &self,
        state: Option<&pb::ConversationStateStructure>,
    ) -> Result<Vec<CanonicalMessage>> {
        let mut messages = Vec::new();
        let Some(state) = state else {
            return Ok(messages);
        };
        for (ordinal, raw_id) in state.root_prompt_messages_json.iter().enumerate() {
            let id = BlobId::from_bytes(raw_id)?;
            let Some(data) = self.sync.get(&id).await? else {
                return Err(Error::Protocol(format!(
                    "missing message Blob {}",
                    id.to_base64()
                )));
            };
            messages.push(messages::decode(
                &data,
                format!("cursor-root:{}:{ordinal}", id.to_base64()),
            )?);
        }
        for message in &messages {
            let MessageContent::ToolResult(result) = &message.content else {
                continue;
            };
            for image in &result.images {
                self.restore_image(&image.blob_id).await?;
            }
        }
        Ok(messages)
    }

    /// Ensures a referenced Tool image exists locally. A recovery on another
    /// database fetches it from Cursor instead of losing the pixels.
    async fn restore_image(&self, blob_id: &str) -> Result<()> {
        let id = BlobId::from_base64(blob_id)?;
        if self.store.get_blob(&id).await?.is_some() {
            return Ok(());
        }
        let data = self
            .sync
            .get(&id)
            .await?
            .ok_or_else(|| Error::Protocol(format!("missing Tool image Blob {blob_id}")))?;
        let stored = self.store.put_blob(&data, &[]).await?;
        if stored != id {
            return Err(Error::Protocol(format!(
                "Tool image Blob hash mismatch: {blob_id}"
            )));
        }
        Ok(())
    }
}
