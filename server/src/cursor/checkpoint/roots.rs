//! Maintains stable append-only Cursor root messages.
use serde_json::Value;

use crate::{
    cursor::checkpoint::messages,
    model::CanonicalMessage,
    store::{BlobEdge, BlobId},
    Error, Result,
};

use super::CheckpointBuilder;

#[derive(Clone)]
pub(super) struct RootFrontier {
    pub(super) ids: Vec<BlobId>,
    pub(super) generated: Vec<Vec<u8>>,
    pub(super) base_count: usize,
}

impl CheckpointBuilder {
    pub(super) async fn project_roots(
        &mut self,
        messages: &[CanonicalMessage],
    ) -> Result<Vec<BlobId>> {
        let wire_messages = messages::stable_messages(&self.instructions, messages, &self.model)?;
        self.ensure_roots()?;
        let replacement = self
            .roots
            .as_ref()
            .and_then(|roots| changed_system_root(roots, &wire_messages));
        if let Some(message) = replacement {
            let id = self.sync.persist(&message, &[]).await?;
            self.roots
                .as_mut()
                .ok_or_else(|| Error::Protocol("Cursor root frontier was not initialized".into()))?
                .ids[0] = id;
        }
        let existing = {
            let roots = self.roots.as_ref().ok_or_else(|| {
                Error::Protocol("Cursor root frontier was not initialized".into())
            })?;
            if wire_messages.len() < roots.ids.len() {
                return Err(Error::Protocol(format!(
                    "Cursor stable history shrank from {} to {} roots",
                    roots.ids.len(),
                    wire_messages.len()
                )));
            }
            for (index, expected) in roots.generated.iter().enumerate() {
                let wire_index = roots.base_count + index;
                if wire_messages.get(wire_index) != Some(expected) {
                    return Err(Error::Protocol(format!(
                        "Cursor stable root changed at index {wire_index}"
                    )));
                }
            }
            roots.ids.len()
        };
        let mut appended = Vec::with_capacity(wire_messages.len() - existing);
        for message in wire_messages.iter().skip(existing) {
            appended.push((self.persist_root(message).await?, message.clone()));
        }
        let roots = self
            .roots
            .as_mut()
            .ok_or_else(|| Error::Protocol("Cursor root frontier was not initialized".into()))?;
        for (id, message) in appended {
            roots.ids.push(id);
            roots.generated.push(message);
        }
        Ok(roots.ids.clone())
    }

    /// Publishes one root message. Image-bearing Tool results first send their
    /// Blobs to Cursor, then link the root to them, so a recovery on another
    /// database can fetch the same pixels.
    async fn persist_root(&self, message: &[u8]) -> Result<BlobId> {
        let mut edges = Vec::new();
        for (index, child) in referenced_image_blobs(message)?.into_iter().enumerate() {
            let data = self.store.get_blob(&child).await?.ok_or_else(|| {
                Error::Protocol(format!("Tool image Blob is missing: {}", child.to_base64()))
            })?;
            self.sync.persist(&data, &[]).await?;
            edges.push(BlobEdge {
                child,
                field_name: format!("tool_result.images[{index}]"),
            });
        }
        self.sync.persist(message, &edges).await
    }

    fn ensure_roots(&mut self) -> Result<()> {
        if self.roots.is_some() {
            return Ok(());
        }
        let ids = self
            .base
            .root_prompt_messages_json
            .iter()
            .map(|id| BlobId::from_bytes(id))
            .collect::<Result<Vec<_>>>()?;
        self.roots = Some(RootFrontier {
            base_count: ids.len(),
            ids,
            generated: Vec::new(),
        });
        Ok(())
    }

    pub(super) async fn replace_roots(
        &mut self,
        messages: &[CanonicalMessage],
    ) -> Result<Vec<BlobId>> {
        let wire_messages = messages::stable_messages(&self.instructions, messages, &self.model)?;
        self.ensure_roots()?;
        let previous_system = self
            .roots
            .as_ref()
            .and_then(|roots| roots.ids.first())
            .cloned();
        let mut ids = Vec::with_capacity(wire_messages.len());
        for (index, message) in wire_messages.iter().enumerate() {
            if index == 0
                && previous_system
                    .as_ref()
                    .is_some_and(|id| *id == BlobId::digest(message))
            {
                ids.push(previous_system.clone().expect("checked system root"));
            } else {
                ids.push(self.persist_root(message).await?);
            }
        }
        self.roots = Some(RootFrontier {
            base_count: ids.len(),
            ids: ids.clone(),
            generated: Vec::new(),
        });
        Ok(ids)
    }
}

fn changed_system_root(roots: &RootFrontier, messages: &[Vec<u8>]) -> Option<Vec<u8>> {
    roots
        .ids
        .first()
        .zip(messages.first())
        .filter(|(current, message)| **current != BlobId::digest(message))
        .map(|(_, message)| message.clone())
}

/// Reads the image Blob references a wire message carries, in order.
fn referenced_image_blobs(message: &[u8]) -> Result<Vec<BlobId>> {
    let value: Value = serde_json::from_slice(message)?;
    if value.get("role").and_then(Value::as_str) != Some("tool") {
        return Ok(Vec::new());
    }
    let mut ids = Vec::new();
    for part in value
        .get("content")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        for image in part
            .pointer("/providerOptions/cursor/images")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            let blob_id = image
                .get("blobId")
                .and_then(Value::as_str)
                .ok_or_else(|| Error::Protocol("Cursor Tool image has no blobId".into()))?;
            ids.push(BlobId::from_base64(blob_id)?);
        }
    }
    Ok(ids)
}
