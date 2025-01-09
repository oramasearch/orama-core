use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
};

use anyhow::{anyhow, Context, Ok, Result};
use tokio::sync::{broadcast::Sender, RwLock};
use tracing::{info, warn};

use crate::{
    collection_manager::dto::CollectionDTO,
    embeddings::EmbeddingService,
    file_utils::{list_directory_in_path, BufferedFile},
    metrics::{AddedDocumentsLabels, ADDED_DOCUMENTS_COUNTER},
    types::{CollectionId, DocumentId, DocumentList},
};

use crate::collection_manager::dto::{CreateCollectionOptionDTO, LanguageDTO};

use super::{collection::CollectionWriter, GenericWriteOperation, WriteOperation};

pub struct CollectionsWriter {
    document_id_generator: Arc<AtomicU64>,
    sender: Sender<WriteOperation>,
    embedding_service: Arc<EmbeddingService>,
    collections: RwLock<HashMap<CollectionId, CollectionWriter>>,
}

impl CollectionsWriter {
    pub fn new(
        sender: Sender<WriteOperation>,
        embedding_service: Arc<EmbeddingService>,
    ) -> CollectionsWriter {
        CollectionsWriter {
            document_id_generator: Default::default(),
            sender,
            embedding_service,
            collections: Default::default(),
        }
    }

    fn generate_document_id(&self) -> DocumentId {
        let id = self
            .document_id_generator
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        DocumentId(id)
    }

    pub async fn create_collection(
        &self,
        collection_option: CreateCollectionOptionDTO,
    ) -> Result<CollectionId> {
        let CreateCollectionOptionDTO {
            id,
            description,
            language,
            typed_fields,
        } = collection_option;

        let id = CollectionId(id);

        self.sender
            .send(WriteOperation::Generic(
                GenericWriteOperation::CreateCollection { id: id.clone() },
            ))
            .context("Cannot send create collection")?;

        let collection = CollectionWriter::new(
            id.clone(),
            description,
            language.unwrap_or(LanguageDTO::English),
        );

        for (field_name, field_type) in typed_fields {
            let field_id = collection.get_field_id_by_name(&field_name);

            collection
                .create_field(
                    field_id,
                    field_name,
                    field_type,
                    self.embedding_service.clone(),
                    &self.sender,
                )
                .await
                .context("Cannot create field")?;
        }

        let mut collections = self.collections.write().await;
        if collections.contains_key(&id) {
            return Err(anyhow!("Collection already exists"));
        }
        collections.insert(id.clone(), collection);
        drop(collections);

        Ok(id)
    }

    pub async fn list(&self) -> Vec<CollectionDTO> {
        let collections = self.collections.read().await;

        collections.iter().map(|(_, coll)| coll.as_dto()).collect()
    }

    pub async fn get_collection_dto(&self, collection_id: CollectionId) -> Option<CollectionDTO> {
        let collections = self.collections.read().await;
        let collection = collections.get(&collection_id);
        collection.map(|c| c.as_dto())
    }

    pub async fn commit(&self, data_dir: PathBuf) -> Result<()> {
        // This `write lock` will not change the content of the collections
        // But it is requered to ensure that the collections are not being modified
        // while we are saving them to disk
        let mut collections = self.collections.write().await;

        std::fs::create_dir_all(&data_dir).context("Cannot create data directory")?;

        let document_id = self.document_id_generator.load(Ordering::Relaxed);
        BufferedFile::create(data_dir.join("document_id"))
            .context("Cannot create document id file")?
            .write_json_data(&document_id)
            .context("Cannot serialize document id")?;

        for (collection_id, collection) in collections.iter_mut() {
            let collection_dir = data_dir.join(collection_id.0.clone());
            collection.commit(collection_dir)?;
        }

        // Now it is safe to drop the lock
        // because we safe everything to disk
        drop(collections);

        Ok(())
    }

    pub async fn load(&mut self, data_dir: PathBuf) -> Result<()> {
        // `&mut self` isn't needed here
        // but we need to ensure that the method is not called concurrently

        let document_id = BufferedFile::open(data_dir.join("document_id"))
            .context("Cannot open document id file")?
            .read_json_data::<u64>()
            .context("Cannot deserialize document id")?;

        self.document_id_generator
            .store(document_id, Ordering::Relaxed);

        let collection_dirs =
            list_directory_in_path(&data_dir).context("Cannot read collection list from disk")?;

        for collection_dir in collection_dirs {
            let file_name = collection_dir
                .file_name()
                .expect("File name is always given at this point");
            let file_name: String = file_name.to_string_lossy().into();

            let collection_id = CollectionId(file_name);

            let mut collection =
                CollectionWriter::new(collection_id.clone(), None, LanguageDTO::English);
            collection
                .load(collection_dir, self.embedding_service.clone())
                .await?;

            self.collections
                .write()
                .await
                .insert(collection_id, collection);
        }

        Ok(())
    }

    pub async fn write(
        &self,
        collection_id: CollectionId,
        document_list: DocumentList,
    ) -> Result<()> {
        info!("Inserting batch of {} documents", document_list.len());
        ADDED_DOCUMENTS_COUNTER
            .create(AddedDocumentsLabels {
                collection: collection_id.0.clone(),
            })
            .increment_by(document_list.len());

        let collections = self.collections.read().await;

        let collection = collections
            .get(&collection_id)
            .ok_or_else(|| anyhow!("Collection not found"))?;

        for mut doc in document_list {
            let doc_id = self.generate_document_id();

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

            collection
                .process_new_document(
                    doc_id,
                    doc,
                    self.embedding_service.clone(),
                    &self.sender.clone(),
                )
                .await
                .context("Cannot process document")?;
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_writer_sync_send() {
        fn assert_sync_send<T: Sync + Send>() {}
        assert_sync_send::<CollectionsWriter>();
    }
}
