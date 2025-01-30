use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{
        atomic::{AtomicU16, AtomicU64},
        Arc,
    },
};

use anyhow::{anyhow, bail, Context, Ok, Result};
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;
use tracing::{debug, field::debug, info, instrument, trace, warn};

use crate::{
    collection_manager::{
        dto::{CollectionDTO, FieldId},
        sides::hooks::{HookName, HooksRuntime},
    },
    file_utils::BufferedFile,
    metrics::{CommitLabels, COMMIT_METRIC},
    nlp::{locales::Locale, NLPService, TextParser},
    types::{CollectionId, ComplexType, Document, DocumentId, ScalarType, ValueType},
};

use crate::collection_manager::dto::{LanguageDTO, TypedField};

use super::{
    embedding::EmbeddingCalculationRequest, CollectionField, CollectionWriteOperation,
    OperationSender, SerializedFieldIndexer, WriteOperation,
};

pub const DEFAULT_EMBEDDING_FIELD_NAME: &str = "___orama_auto_embedding";

pub struct CollectionWriter {
    id: CollectionId,
    description: Option<String>,
    default_language: LanguageDTO,
    fields: RwLock<HashMap<FieldId, (String, ValueType, CollectionField)>>,

    collection_document_count: AtomicU64,

    field_id_generator: AtomicU16,
    field_id_by_name: RwLock<HashMap<String, FieldId>>,

    embedding_sender: tokio::sync::mpsc::Sender<EmbeddingCalculationRequest>,
}

impl CollectionWriter {
    pub fn new(
        id: CollectionId,
        description: Option<String>,
        default_language: LanguageDTO,
        embedding_sender: tokio::sync::mpsc::Sender<EmbeddingCalculationRequest>,
    ) -> Self {
        Self {
            id: id.clone(),
            description,
            default_language,
            collection_document_count: Default::default(),
            fields: Default::default(),
            field_id_by_name: Default::default(),
            field_id_generator: Default::default(),
            embedding_sender,
        }
    }

    pub async fn as_dto(&self) -> CollectionDTO {
        let fields = self.fields.read().await;
        let fields = fields
            .iter()
            .map(|(_, (key, v, _))| (key.clone(), v.clone()))
            .collect();

        CollectionDTO {
            id: self.id.clone(),
            description: self.description.clone(),
            document_count: self
                .collection_document_count
                .load(std::sync::atomic::Ordering::Relaxed),
            fields,
        }
    }

    pub async fn set_embedding_hook(&self, hook_name: HookName) -> Result<()> {
        let field_id_by_name = self.field_id_by_name.read().await;
        let field_id = field_id_by_name
            .get(DEFAULT_EMBEDDING_FIELD_NAME)
            .cloned()
            .context("Field for embedding not found")?;
        drop(field_id_by_name);

        let mut w = self.fields.write().await;
        let field = match w.get_mut(&field_id) {
            None => bail!("Field for embedding not found"),
            Some((_, _, field)) => field,
        };
        field.set_embedding_hook(hook_name);

        Ok(())
    }

    pub async fn process_new_document(
        &self,
        doc_id: DocumentId,
        doc: Document,
        sender: OperationSender,
        hooks_runtime: Arc<HooksRuntime>,
    ) -> Result<()> {
        self.collection_document_count
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);

        sender
            .send(WriteOperation::Collection(
                self.id.clone(),
                CollectionWriteOperation::InsertDocument {
                    doc_id,
                    doc: doc.into_raw()?,
                },
            ))
            .await
            .map_err(|e| anyhow!("Error sending document to index writer: {:?}", e))?;

        let fields_to_index = self
            .get_fields_to_index(doc.clone(), sender.clone(), hooks_runtime)
            .await
            .context("Cannot get fields to index")?;
        trace!("Fields to index: {:?}", fields_to_index);

        let flatten = doc.clone().into_flatten();

        let r = self.fields.read().await;
        for field_id in fields_to_index {
            let (field_name, _, field) = match r.get(&field_id) {
                None => {
                    info!("Field not indexed");
                    continue;
                }
                Some(v) => v,
            };

            info!("QUIQUIQUI {:?}", field_name);
            field
                .get_write_operations(doc_id, &flatten, sender.clone())
                .await
                .with_context(|| format!("Cannot index field {}", field_name))?;
            info!("QUAQUAQUA");
        }

        trace!("Document field indexed");

        Ok(())
    }

    fn value_to_typed_field(&self, value_type: ValueType) -> Option<TypedField> {
        match value_type {
            ValueType::Scalar(ScalarType::String) => {
                Some(TypedField::Text(self.default_language.into()))
            }
            ValueType::Scalar(ScalarType::Number) => Some(TypedField::Number),
            ValueType::Scalar(ScalarType::Boolean) => Some(TypedField::Bool),
            _ => None, // @todo: support other types
        }
    }

    fn get_text_parser(&self, locale: Locale) -> Arc<TextParser> {
        // TextParser is expensive to create, so we cache it
        // TODO: add a cache
        let parser = TextParser::from_locale(locale);
        Arc::new(parser)
    }

    pub async fn register_fields(
        &self,
        typed_fields: HashMap<String, TypedField>,
        sender: OperationSender,
        hooks_runtime: Arc<HooksRuntime>,
    ) -> Result<()> {
        for (field_name, field_type) in typed_fields {
            info!(
                "Registering field {} with type {:?}",
                field_name, field_type
            );

            let field_id = self
                .field_id_by_name
                .read()
                .await
                .get(&field_name)
                .map(|e| *e);
            // Avoid creating fields that already exists
            if field_id.is_some() {
                continue;
            }

            let field_id = self
                .field_id_generator
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let field_id = FieldId(field_id);
            let mut field_id_by_name = self.field_id_by_name.write().await;
            field_id_by_name.insert(field_name.clone(), field_id);
            drop(field_id_by_name);

            self.create_field(
                field_id,
                field_name,
                field_type,
                self.embedding_sender.clone(),
                sender.clone(),
                hooks_runtime.clone(),
            )
            .await
            .context("Cannot create field")?;
        }

        Ok(())
    }

    #[instrument(skip(self, field_id, sender, embedding_sender, hooks_runtime))]
    async fn create_field(
        &self,
        field_id: FieldId,
        field_name: String,
        typed_field: TypedField,
        embedding_sender: tokio::sync::mpsc::Sender<EmbeddingCalculationRequest>,
        sender: OperationSender,
        hooks_runtime: Arc<HooksRuntime>,
    ) -> Result<()> {
        let mut w = self.fields.write().await;
        match &typed_field {
            TypedField::Embedding(embedding_field) => {
                w.insert(
                    field_id,
                    (
                        field_name.clone(),
                        ValueType::Complex(ComplexType::Embedding),
                        CollectionField::new_embedding(
                            embedding_field.model,
                            embedding_field.document_fields.clone(),
                            embedding_sender,
                            hooks_runtime,
                            self.id.clone(),
                            field_id,
                        ),
                    ),
                );
            }
            TypedField::Text(locale) => {
                let parser = self.get_text_parser(*locale);
                w.insert(
                    field_id,
                    (
                        field_name.clone(),
                        ValueType::Scalar(ScalarType::String),
                        CollectionField::new_string(
                            parser,
                            self.id.clone(),
                            field_id,
                            field_name.clone(),
                        ),
                    ),
                );
            }
            TypedField::Number => {
                w.insert(
                    field_id,
                    (
                        field_name.clone(),
                        ValueType::Scalar(ScalarType::Number),
                        CollectionField::new_number(self.id.clone(), field_id, field_name.clone()),
                    ),
                );
            }
            TypedField::Bool => {
                w.insert(
                    field_id,
                    (
                        field_name.clone(),
                        ValueType::Scalar(ScalarType::Boolean),
                        CollectionField::new_bool(self.id.clone(), field_id, field_name.clone()),
                    ),
                );
            }
        }
        drop(w);

        sender
            .send(WriteOperation::Collection(
                self.id.clone(),
                CollectionWriteOperation::CreateField {
                    field_id,
                    field_name,
                    field: typed_field,
                },
            ))
            .await
            .context("Cannot sent creation field")?;
        info!("Field created");

        Ok(())
    }

    #[instrument(skip(self, doc, sender, hooks_runtime))]
    async fn get_fields_to_index(
        &self,
        doc: Document,
        sender: OperationSender,
        hooks_runtime: Arc<HooksRuntime>,
    ) -> Result<Vec<FieldId>> {
        let flatten = doc.clone().into_flatten();
        let schema = flatten.get_field_schema();

        let mut field_ids = vec![];
        let field_id_by_name = self.field_id_by_name.read().await;
        if let Some(field_id) = field_id_by_name.get(DEFAULT_EMBEDDING_FIELD_NAME) {
            field_ids.push(field_id.clone());
        }
        drop(field_id_by_name);

        for (field_name, value_type) in schema {
            info!("1");
            let field_id_by_name = self.field_id_by_name.read().await;
            let field_id = field_id_by_name.get(&field_name).cloned();
            drop(field_id_by_name);

            if let Some(field_id) = field_id {
                info!("3");
                field_ids.push(field_id);
                continue;
            }

            let field_id = self
                .field_id_generator
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let field_id = FieldId(field_id);
            info!("2");
            let mut field_id_by_name = self.field_id_by_name.write().await;
            field_id_by_name.insert(field_name.clone(), field_id);
            drop(field_id_by_name);
            info!("2222");

            field_ids.push(field_id);

            // @todo: add support to other types
            if let Some(typed_field) = self.value_to_typed_field(value_type.clone()) {
                info!("4");
                self.create_field(
                    field_id,
                    field_name,
                    typed_field,
                    self.embedding_sender.clone(),
                    sender.clone(),
                    hooks_runtime.clone(),
                )
                .await
                .context("Cannot create field")?;
            } else {
                warn!("Field type not supported: {:?}", value_type);
            }

            info!("5");
        }

        trace!("field_ids: {:?}", field_ids);

        Ok(field_ids)
    }

    pub async fn commit(&self, path: PathBuf) -> Result<()> {
        info!("Committing collection {}", self.id.0);

        let m = COMMIT_METRIC.create(CommitLabels {
            side: "write",
            collection: self.id.0.clone(),
            index_type: "info",
        });
        std::fs::create_dir_all(&path).context("Cannot create collection directory")?;

        let fields = self.fields.read().await;
        let fields = fields
            .iter()
            .map(|(_, (k, _, indexer))| (k.clone(), indexer.serialized()))
            .collect();

        let field_id_by_name = self.field_id_by_name.read().await;
        let field_id_by_name: Vec<_> = field_id_by_name
            .iter()
            .map(|(k, v)| (k.clone(), *v))
            .collect();

        let dump = CollectionDump::V1(CollectionDumpV1 {
            id: self.id.clone(),
            description: self.description.clone(),
            default_language: self.default_language,
            fields,
            document_count: self
                .collection_document_count
                .load(std::sync::atomic::Ordering::Relaxed),
            field_id_generator: self
                .field_id_generator
                .load(std::sync::atomic::Ordering::Relaxed),
            field_id_by_name,
        });

        BufferedFile::create_or_overwrite(path.join("info.json"))
            .context("Cannot create info.json file")?
            .write_json_data(&dump)
            .context("Cannot serialize collection info")?;

        drop(m);

        Ok(())
    }

    pub async fn load(
        &mut self,
        path: PathBuf,
        hooks_runtime: Arc<HooksRuntime>,
        nlp_service: Arc<NLPService>,
    ) -> Result<()> {
        let dump: CollectionDump = BufferedFile::open(path.join("info.json"))
            .context("Cannot open info.json file")?
            .read_json_data()
            .context("Cannot deserialize collection info")?;

        let CollectionDump::V1(dump) = dump;

        self.id = dump.id;
        self.description = dump.description;
        self.default_language = dump.default_language;
        self.field_id_by_name = RwLock::new(dump.field_id_by_name.into_iter().collect());

        for (field_name, serialized) in dump.fields {
            let field_id_by_name = self.field_id_by_name.read().await;
            let field_id = match field_id_by_name.get(&field_name) {
                None => {
                    return Err(anyhow!(
                        "Field {} not found in field_id_by_name",
                        field_name
                    ));
                }
                Some(field_id) => *field_id,
            };
            drop(field_id_by_name);

            let (value_type, collection_field): (ValueType, CollectionField) = match serialized {
                SerializedFieldIndexer::String(locale) => (
                    ValueType::Scalar(ScalarType::String),
                    CollectionField::new_string(
                        nlp_service.get(locale),
                        self.id.clone(),
                        field_id,
                        field_name.clone(),
                    ),
                ),
                SerializedFieldIndexer::Number => (
                    ValueType::Scalar(ScalarType::Number),
                    CollectionField::new_number(self.id.clone(), field_id, field_name.clone()),
                ),
                SerializedFieldIndexer::Bool => (
                    ValueType::Scalar(ScalarType::Boolean),
                    CollectionField::new_bool(self.id.clone(), field_id, field_name.clone()),
                ),
                SerializedFieldIndexer::Embedding(model, fields) => (
                    ValueType::Complex(ComplexType::Embedding),
                    CollectionField::new_embedding(
                        model.0,
                        fields,
                        self.embedding_sender.clone(),
                        hooks_runtime.clone(),
                        self.id.clone(),
                        field_id,
                    ),
                ),
            };
            let mut w = self.fields.write().await;
            w.insert(field_id, (field_name, value_type, collection_field));
        }
        self.collection_document_count
            .store(dump.document_count, std::sync::atomic::Ordering::Relaxed);
        self.field_id_generator = AtomicU16::new(dump.field_id_generator);

        Ok(())
    }
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "version")]
enum CollectionDump {
    #[serde(rename = "1")]
    V1(CollectionDumpV1),
}

#[derive(Debug, Serialize, Deserialize)]
struct CollectionDumpV1 {
    id: CollectionId,
    description: Option<String>,
    default_language: LanguageDTO,
    fields: Vec<(String, SerializedFieldIndexer)>,
    document_count: u64,
    field_id_generator: u16,
    field_id_by_name: Vec<(String, FieldId)>,
}
