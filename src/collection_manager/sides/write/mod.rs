mod collection;
mod collections;
mod embedding;
mod fields;
mod operation;

use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
};

use super::hooks::{HookName, HooksRuntime};
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use tracing::{info, warn};

use collections::CollectionsWriter;
pub use collections::CollectionsWriterConfig;
use embedding::{start_calculate_embedding_loop, EmbeddingCalculationRequest};
pub use operation::*;

#[cfg(any(test, feature = "benchmarking"))]
pub use fields::*;

use crate::{
    ai::AIService,
    collection_manager::dto::{CollectionDTO, CreateCollection},
    file_utils::BufferedFile,
    metrics::{AddedDocumentsLabels, ADDED_DOCUMENTS_COUNTER},
    nlp::NLPService,
    types::{CollectionId, DocumentId, DocumentList},
};

pub struct WriteSide {
    sender: OperationSender,
    collections: CollectionsWriter,
    document_count: AtomicU64,
    data_dir: PathBuf,
    hook_runtime: Arc<HooksRuntime>,
    nlp_service: Arc<NLPService>,
}

impl WriteSide {
    pub fn new(
        sender: OperationSender,
        config: CollectionsWriterConfig,
        ai_service: Arc<AIService>,
        hook_runtime: Arc<HooksRuntime>,
        nlp_service: Arc<NLPService>,
    ) -> WriteSide {
        let data_dir = config.data_dir.clone();

        let (sx, rx) =
            tokio::sync::mpsc::channel::<EmbeddingCalculationRequest>(config.embedding_queue_limit);

        start_calculate_embedding_loop(ai_service, rx, config.embedding_queue_limit);

        WriteSide {
            sender,
            collections: CollectionsWriter::new(config, sx),
            document_count: AtomicU64::new(0),
            data_dir,
            hook_runtime,
            nlp_service,
        }
    }

    pub async fn load(&mut self) -> Result<()> {
        self.collections
            .load(self.hook_runtime.clone(), self.nlp_service.clone())
            .await?;

        let info: WriteSideInfo = match BufferedFile::open(self.data_dir.join("info.json"))
            .and_then(|f| f.read_json_data())
            .context("Cannot read info file")
        {
            Ok(info) => info,
            Err(err) => {
                warn!("Cannot read info file: {}. Skip loading", err);
                return Ok(());
            }
        };
        let info = match info {
            WriteSideInfo::V1(info) => info,
        };

        self.document_count
            .store(info.document_count, Ordering::Relaxed);
        self.sender.set_offset(info.offset);

        Ok(())
    }

    pub async fn commit(&self) -> Result<()> {
        let offset = self.sender.offset();

        self.collections.commit().await?;

        // This load is not atomic with the commit.
        // This means, we save a document count possible higher.
        // Anyway it is not a problem, because the document count is only used for the document id generation
        // So, if something goes wrong, we save an higher number, and this is ok.
        let document_count = self.document_count.load(Ordering::Relaxed);
        let info = WriteSideInfo::V1(WriteSideInfoV1 {
            document_count,
            offset,
        });
        BufferedFile::create_or_overwrite(self.data_dir.join("info.json"))
            .context("Cannot create info file")?
            .write_json_data(&info)
            .context("Cannot write info file")?;

        Ok(())
    }

    pub async fn create_collection(&self, option: CreateCollection) -> Result<()> {
        self.collections
            .create_collection(option, self.sender.clone(), self.hook_runtime.clone())
            .await?;

        Ok(())
    }

    pub async fn write(
        &self,
        collection_id: CollectionId,
        document_list: DocumentList,
    ) -> Result<()> {
        info!("Inserting batch of {} documents", document_list.len());

        // This counter is not atomic with the insert operation.
        // This means we increment the counter even if the insert operation fails.
        // TODO: think better
        ADDED_DOCUMENTS_COUNTER
            .create(AddedDocumentsLabels {
                collection: collection_id.0.clone(),
            })
            .increment_by(document_list.len());

        let collection = self
            .collections
            .get_collection(collection_id.clone())
            .await
            .ok_or_else(|| anyhow::anyhow!("Collection not found"))?;

        let sender = self.sender.clone();

        for mut doc in document_list {
            let doc_id = self.document_count.fetch_add(1, Ordering::Relaxed);

            let doc_id_value = doc.get("id");
            // Forces the id to be set, if not set
            if doc_id_value.is_none() {
                doc.inner.insert(
                    "id".to_string(),
                    serde_json::Value::String(cuid2::create_id()),
                );
            } else if let Some(doc_id_value) = doc_id_value {
                if !doc_id_value.is_string() {
                    // The search result contains the document id and it is defined as a string.
                    // So, if the original document id is not a string, we should overwrite it with a new one
                    // Anyway, this implies the loss of the original document id. For instance we could support number as well
                    // TODO: think better
                    warn!("Document id is not a string, overwriting it with new one");
                    doc.inner.insert(
                        "id".to_string(),
                        serde_json::Value::String(cuid2::create_id()),
                    );
                }
            }

            let doc_id = DocumentId(doc_id);
            collection
                .process_new_document(doc_id, doc, sender.clone(), self.hook_runtime.clone())
                .await
                .context("Cannot process document")?;
        }

        Ok(())
    }

    pub async fn insert_javascript_hook(
        &self,
        collection_id: CollectionId,
        name: HookName,
        code: String,
    ) -> Result<()> {
        self.hook_runtime
            .insert_hook(collection_id.clone(), name.clone(), code)
            .context("Cannot insert hook")?;

        let collection = self
            .collections
            .get_collection(collection_id)
            .await
            .ok_or_else(|| anyhow::anyhow!("Collection not found"))?;

        collection.set_embedding_hook(name);

        Ok(())
    }

    pub async fn list_collections(&self) -> Vec<CollectionDTO> {
        self.collections.list().await
    }

    pub async fn get_collection_dto(&self, collection_id: CollectionId) -> Option<CollectionDTO> {
        let collection = self.collections.get_collection(collection_id).await?;
        Some(collection.as_dto())
    }

    pub fn get_javascript_hook(
        &self,
        collection_id: CollectionId,
        name: HookName,
    ) -> Option<String> {
        self.hook_runtime
            .get_hook(collection_id, name)
            .map(|hook| hook.code)
    }

    pub fn delete_javascript_hook(
        &self,
        _collection_id: CollectionId,
        _name: HookName,
    ) -> Option<String> {
        None // @todo: implement delete hook in HooksRuntime and CollectionsWriter
    }

    pub fn list_javascript_hooks(&self, collection_id: CollectionId) -> HashMap<HookName, String> {
        self.hook_runtime
            .list_hooks(collection_id)
            .into_iter()
            .map(|(name, hook)| (name, hook.code))
            .collect()
    }
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "version")]
enum WriteSideInfo {
    #[serde(rename = "1")]
    V1(WriteSideInfoV1),
}

#[derive(Serialize, Deserialize)]
struct WriteSideInfoV1 {
    document_count: u64,
    offset: Offset,
}
