use std::{
    collections::{HashMap, HashSet},
    path::PathBuf,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
};

use anyhow::{anyhow, Context, Result};
use committed::CommittedCollection;
use dashmap::DashMap;
use dump::{CollectionInfo, CollectionInfoV1};
use merge::{merge_bool_field, merge_number_field, merge_string_field, merge_vector_field};
use serde::{Deserialize, Serialize};
use tokio::{
    join,
    sync::{Mutex, RwLock},
};
use tracing::{debug, error, info, instrument, trace, warn};
use uncommitted::UncommittedCollection;

mod committed;
mod merge;
mod uncommitted;

use crate::{
    ai::{AIService, OramaModel},
    collection_manager::{
        dto::{
            self, EmbeddingTypedField, FacetDefinition, FacetResult, FieldId, Filter, Limit,
            Properties, SearchMode, SearchParams,
        },
        sides::{CollectionWriteOperation, Offset, OramaModelSerializable},
    },
    file_utils::BufferedFile,
    indexes::{number::NumberFilter, string::BM25Scorer},
    metrics::{
        SearchFilterLabels, SearchLabels, SEARCH_FILTER_HISTOGRAM, SEARCH_FILTER_METRIC,
        SEARCH_METRIC,
    },
    nlp::{locales::Locale, NLPService, TextParser},
    offset_storage::OffsetStorage,
    types::{CollectionId, DocumentId},
};

use super::IndexesConfig;

#[derive(Debug)]
pub struct CollectionReader {
    id: CollectionId,
    ai_service: Arc<AIService>,
    nlp_service: Arc<NLPService>,

    document_count: AtomicU64,

    fields: DashMap<String, (FieldId, TypedField)>,

    uncommitted_collection: RwLock<UncommittedCollection>,
    committed_collection: RwLock<CommittedCollection>,

    fields_per_model: DashMap<OramaModel, Vec<FieldId>>,

    text_parser_per_field: DashMap<FieldId, (Locale, Arc<TextParser>)>,

    offset_storage: OffsetStorage,
    commit_insert_mutex: Mutex<()>,
}

impl CollectionReader {
    pub fn try_new(
        id: CollectionId,
        ai_service: Arc<AIService>,
        nlp_service: Arc<NLPService>,
        _: IndexesConfig,
    ) -> Result<Self> {
        Ok(Self {
            id,
            ai_service,
            nlp_service,
            document_count: AtomicU64::new(0),
            fields_per_model: Default::default(),
            text_parser_per_field: Default::default(),
            fields: Default::default(),

            uncommitted_collection: RwLock::new(UncommittedCollection::new()),
            committed_collection: RwLock::new(CommittedCollection::new()),

            offset_storage: Default::default(),
            commit_insert_mutex: Default::default(),
        })
    }

    #[inline]
    pub fn get_id(&self) -> CollectionId {
        self.id.clone()
    }

    pub fn get_field_id(&self, field_name: String) -> Result<FieldId> {
        let field_id = self.fields.get(&field_name);

        match field_id {
            Some(field_id) => Ok(field_id.0),
            None => Err(anyhow!("Field not found")),
        }
    }

    pub fn get_field_id_with_type(&self, field_name: &str) -> Result<(FieldId, TypedField)> {
        self.fields
            .get(field_name)
            .map(|v| v.clone())
            .ok_or_else(|| anyhow!("Field not found"))
    }

    pub async fn load(&mut self, data_dir: PathBuf) -> Result<()> {
        let collection_info_path = data_dir.join("info.info");
        let previous_offset: Offset = match BufferedFile::open(collection_info_path.clone())
            .context("Cannot open previous collection info")?
            .read_json_data()
        {
            Ok(offset) => offset,
            Err(e) => {
                warn!("Cannot read previous collection info from {:?} due to {:?}. Skip loading collection", collection_info_path, e);
                return Ok(());
            }
        };

        let collection_info_path = data_dir.join(format!("info-offset-{}.info", previous_offset.0));
        let collection_info: CollectionInfo = BufferedFile::open(collection_info_path)
            .context("Cannot open previous collection info")?
            .read_json_data()
            .context("Cannot read previous collection info")?;

        let dump::CollectionInfo::V1(collection_info) = collection_info;

        for (field_name, (field_id, field_type)) in collection_info.fields {
            let typed_field: TypedField = match field_type {
                dump::TypedField::Text(locale) => TypedField::Text(locale),
                dump::TypedField::Embedding(embedding) => TypedField::Embedding(embedding.model.0),
                dump::TypedField::Number => TypedField::Number,
                dump::TypedField::Bool => TypedField::Bool,
            };
            self.fields.insert(field_name, (field_id, typed_field));
        }

        for (orama_model, fields) in collection_info.used_models {
            self.fields_per_model.insert(orama_model.0, fields);
        }

        self.text_parser_per_field = self
            .fields
            .iter()
            .filter_map(|e| {
                if let TypedField::Text(l) = e.1 {
                    let locale = l.into();
                    Some((e.0, (locale, self.nlp_service.get(locale))))
                } else {
                    None
                }
            })
            .collect();

        let mut lock = self.committed_collection.write().await;
        lock.load(
            collection_info.number_field_infos,
            collection_info.bool_field_infos,
            collection_info.string_field_infos,
        )?;

        /*
        self.string_index
            .load(collection_data_dir.join("strings"))
            .context("Cannot load string index")?;
        self.number_index
            .load(collection_data_dir.join("numbers"))
            .context("Cannot load number index")?;
        self.vector_index
            .load(collection_data_dir.join("vectors"))
            .context("Cannot load vectors index")?;
        self.bool_index
            .load(collection_data_dir.join("bools"))
            .context("Cannot load bool index")?;

        let coll_desc_file_path = collection_data_dir.join("info.json");
        let dump: dump::CollectionInfo = BufferedFile::open(coll_desc_file_path)
            .context("Cannot open collection file")?
            .read_json_data()
            .with_context(|| format!("Cannot deserialize collection info for {:?}", self.id))?;

        let dump::CollectionInfo::V1(dump) = dump;

        for (field_name, (field_id, field_type)) in dump.fields {
            let typed_field: TypedField = match field_type {
                dump::TypedField::Text(language) => TypedField::Text(language),
                dump::TypedField::Embedding(embedding) => {
                    TypedField::Embedding(EmbeddingTypedField {
                        document_fields: embedding.document_fields,
                        model: embedding.model.0,
                    })
                }
                dump::TypedField::Number => TypedField::Number,
                dump::TypedField::Bool => TypedField::Bool,
            };
            self.fields.insert(field_name, (field_id, typed_field));
        }

        for (orama_model, fields) in dump.used_models {
            self.fields_per_model.insert(orama_model.0, fields);
        }

        self.text_parser_per_field = self
            .fields
            .iter()
            .filter_map(|e| {
                if let TypedField::Text(l) = e.1 {
                    let locale = l.into();
                    Some((e.0, (locale, self.nlp_service.get(locale))))
                } else {
                    None
                }
            })
            .collect();
        */

        Ok(())
    }

    #[instrument(skip(self, data_dir), fields(self.id = ?self.id))]
    pub async fn commit(&self, data_dir: PathBuf) -> Result<()> {
        info!("Committing collection");

        // We stop insertion operations while we are committing
        let commit_insert_mutex_lock = self.commit_insert_mutex.lock().await;

        let offset = self.offset_storage.get_offset();

        let collection_info_path = data_dir.join("info.info");
        let previous_offset: Option<Offset> = match BufferedFile::open(collection_info_path.clone())
            .context("Cannot open previous collection info")?
            .read_json_data()
        {
            Ok(offset) => Some(offset),
            Err(_) => None,
        };

        let previous_offset_collection_info_path = previous_offset.map(|previous_offset| {
            data_dir.join(format!("info-offset-{}.info", previous_offset.0))
        });
        let mut current_collection_info = if let Some(previous_offset_collection_info_path) =
            previous_offset_collection_info_path
        {
            let previous_collection_info: CollectionInfo =
                BufferedFile::open(previous_offset_collection_info_path)
                    .context("Cannot open previous collection info")?
                    .read_json_data()
                    .context("Cannot read previous collection info")?;

            match previous_collection_info {
                CollectionInfo::V1(info) => info,
            }
        } else {
            CollectionInfoV1 {
                fields: Default::default(),
                id: self.id.clone(),
                used_models: Default::default(),
                number_field_infos: Default::default(),
                string_field_infos: Default::default(),
                bool_field_infos: Default::default(),
                vector_field_infos: Default::default(),
            }
        };

        let committed = self.committed_collection.read().await;
        let uncommitted = self.uncommitted_collection.read().await;

        let mut number_fields = HashMap::new();
        let number_dir = data_dir.join("numbers");
        for field_id in uncommitted.get_number_fields() {
            let uncommitted_number_index = uncommitted.number_index.get(&field_id).unwrap();
            let committed_number_index = committed.number_index.get(&field_id);

            let field_dir = number_dir
                .join(format!("field-{}", field_id.0))
                .join(format!("offset-{}", offset.0));
            let new_committed_number_index =
                merge_number_field(uncommitted_number_index, committed_number_index, field_dir)
                    .with_context(|| {
                        format!(
                            "Cannot merge {:?} field for collection {:?}",
                            field_id, self.id
                        )
                    })?;
            let field_info = new_committed_number_index.get_field_info();
            current_collection_info.number_field_infos = current_collection_info
                .number_field_infos
                .into_iter()
                .filter(|(k, _)| k != field_id)
                .collect();
            current_collection_info
                .number_field_infos
                .push((*field_id, field_info));

            number_fields.insert(*field_id, new_committed_number_index);

            let field = current_collection_info
                .fields
                .iter_mut()
                .find(|(_, (field_id, _))| field_id == field_id);
            match field {
                Some((_, (_, typed_field))) => {
                    if typed_field != &mut dump::TypedField::Number {
                        error!("Field {:?} is changing type and this is not allowed. before {:?} after {:?}", field_id, typed_field, dump::TypedField::Number);
                        return Err(anyhow!(
                            "Field {:?} is changing type and this is not allowed",
                            field_id
                        ));
                    }
                }
                None => {
                    let field_name = self
                        .fields
                        .iter()
                        .find(|e| e.0 == *field_id)
                        .context("Field not registered")?;
                    let field_name = field_name.key().to_string();
                    current_collection_info
                        .fields
                        .push((field_name, (*field_id, dump::TypedField::Number)));
                }
            }
        }

        let mut string_fields = HashMap::new();
        let string_dir = data_dir.join("strings");
        for field_id in uncommitted.get_string_fields() {
            let uncommitted_string_index = uncommitted.string_index.get(&field_id).unwrap();
            let committed_string_index = committed.string_index.get(&field_id);

            let field_dir = string_dir
                .join(format!("field-{}", field_id.0))
                .join(format!("offset-{}", offset.0));
            let new_committed_string_index =
                merge_string_field(uncommitted_string_index, committed_string_index, field_dir)
                    .with_context(|| {
                        format!(
                            "Cannot merge {:?} field for collection {:?}",
                            field_id, self.id
                        )
                    })?;
            let field_info = new_committed_string_index.get_field_info();
            string_fields.insert(*field_id, new_committed_string_index);
            current_collection_info.string_field_infos = current_collection_info
                .string_field_infos
                .into_iter()
                .filter(|(k, _)| k != field_id)
                .collect();
            current_collection_info
                .string_field_infos
                .push((*field_id, field_info));

            let field_locale = self
                .text_parser_per_field
                .get(&field_id)
                .map(|e| e.0)
                .context("String field not registered")?;
            let field = current_collection_info
                .fields
                .iter_mut()
                .find(|(_, (f, _))| f == field_id);
            match field {
                Some((_, (_, typed_field))) => {
                    if typed_field != &mut dump::TypedField::Text(field_locale) {
                        error!("Field {:?} is changing type and this is not allowed. before {:?} after {:?}", field_id, typed_field, dump::TypedField::Text(field_locale));
                        return Err(anyhow!(
                            "Field {:?} is changing type and this is not allowed",
                            field_id
                        ));
                    }
                }
                None => {
                    let field_name = self
                        .fields
                        .iter()
                        .find(|e| e.0 == *field_id)
                        .context("Field not registered")?;
                    let field_name = field_name.key().to_string();
                    current_collection_info.fields.push((
                        field_name,
                        (*field_id, dump::TypedField::Text(field_locale)),
                    ));
                }
            }
        }

        let mut bool_fields = HashMap::new();
        let bool_dir = data_dir.join("bools");
        for field_id in uncommitted.get_bool_fields() {
            let uncommitted_bool_index = uncommitted.bool_index.get(&field_id).unwrap();
            let committed_bool_index = committed.bool_index.get(&field_id);

            let field_dir = bool_dir
                .join(format!("field-{}", field_id.0))
                .join(format!("offset-{}", offset.0));
            let new_committed_bool_index =
                merge_bool_field(uncommitted_bool_index, committed_bool_index, field_dir)
                    .with_context(|| {
                        format!(
                            "Cannot merge {:?} field for collection {:?}",
                            field_id, self.id
                        )
                    })?;
            let field_info = new_committed_bool_index.get_field_info();
            bool_fields.insert(*field_id, new_committed_bool_index);
            current_collection_info.bool_field_infos = current_collection_info
                .bool_field_infos
                .into_iter()
                .filter(|(k, _)| k != field_id)
                .collect();
            current_collection_info
                .bool_field_infos
                .push((*field_id, field_info));

            let field = current_collection_info
                .fields
                .iter_mut()
                .find(|(_, (f, _))| f == field_id);
            match field {
                Some((_, (_, typed_field))) => {
                    if typed_field != &mut dump::TypedField::Bool {
                        error!("Field {:?} is changing type and this is not allowed. before {:?} after {:?}", field_id, typed_field, dump::TypedField::Bool);
                        return Err(anyhow!(
                            "Field {:?} is changing type and this is not allowed",
                            field_id
                        ));
                    }
                }
                None => {
                    let field_name = self
                        .fields
                        .iter()
                        .find(|e| e.0 == *field_id)
                        .context("Field not registered")?;
                    let field_name = field_name.key().to_string();
                    current_collection_info
                        .fields
                        .push((field_name, (*field_id, dump::TypedField::Bool)));
                }
            }
        }

        let mut vector_fields = HashMap::new();
        let vector_dir = data_dir.join("vectors");
        for field_id in uncommitted.get_vector_fields() {
            let uncommitted_vector_index = uncommitted.vector_index.get(&field_id).unwrap();
            let committed_vector_index = committed.vector_index.get(&field_id);

            let field_dir = bool_dir
                .join(format!("field-{}", field_id.0))
                .join(format!("offset-{}", offset.0));
            let new_committed_vector_index =
                merge_vector_field(uncommitted_vector_index, committed_vector_index, field_dir)
                    .with_context(|| {
                        format!(
                            "Cannot merge {:?} field for collection {:?}",
                            field_id, self.id
                        )
                    })?;
            let field_info = new_committed_vector_index.get_field_info();
            vector_fields.insert(*field_id, new_committed_vector_index);
            current_collection_info.vector_field_infos = current_collection_info
                .vector_field_infos
                .into_iter()
                .filter(|(k, _)| k != field_id)
                .collect();
            current_collection_info
                .vector_field_infos
                .push((*field_id, field_info));

            let field = current_collection_info
                .fields
                .iter_mut()
                .find(|(_, (f, _))| f == field_id);
            match field {
                Some((_, (_, typed_field))) => {
                    // TODO: check if the field is changing type
                }
                None => {
                    let field_name = self
                        .fields
                        .iter()
                        .find(|e| e.0 == *field_id)
                        .context("Field not registered")?;
                    let field_name = field_name.key().to_string();

                    let item = self
                        .fields_per_model
                        .iter()
                        .find(|e| e.value().contains(field_id))
                        .context("Field not registered")?;
                    let orama_model = item.key();

                    current_collection_info.fields.push((
                        field_name,
                        (
                            *field_id,
                            dump::TypedField::Embedding(dump::EmbeddingTypedField {
                                model: OramaModelSerializable(*orama_model),
                            }),
                        ),
                    ));
                }
            }
        }

        // Read lock ends
        drop(committed);
        drop(uncommitted);

        // The following loop should be fast, so the read lock is not held for a long time
        let (mut committed, mut uncommitted) = join!(
            self.committed_collection.write(),
            self.uncommitted_collection.write()
        );
        for (field_id, field) in number_fields {
            uncommitted.number_index.remove(&field_id);
            committed.number_index.insert(field_id, field);
        }
        for (field_id, field) in string_fields {
            uncommitted.string_index.remove(&field_id);
            committed.string_index.insert(field_id, field);
        }
        drop(committed);
        drop(uncommitted);

        let new_offset_collection_info_path =
            data_dir.join(format!("info-offset-{}.info", offset.0));
        BufferedFile::create(new_offset_collection_info_path)
            .context("Cannot create previous collection info")?
            .write_json_data(&CollectionInfo::V1(current_collection_info))
            .context("Cannot write previous collection info")?;

        BufferedFile::create_or_overwrite(collection_info_path)
            .context("Cannot create previous collection info")?
            .write_json_data(&offset)
            .context("Cannot write previous collection info")?;

        drop(commit_insert_mutex_lock);

        /*

            let data_dir = data_dir.join("fields");

        let number_dir = data_dir.join("numbers");
        for (field_id, field) in &uncommitted.number_index {
            let committed = committed.number_index.get(field_id);

            let field_dir = number_dir.join(format!("field-{}", field_id.0));

            let new_committed_field = merge_number_field(field, committed, field_dir)?;

            committed.number_index.insert(*field_id, new_committed_field);
        }

        let bool_dir = data_dir.join("bools");
        for (field_id, field) in &uncommitted.bool_index {
            let committed = committed.bool_index.get(field_id);

            let field_dir = bool_dir.join(format!("field-{}", field_id.0));

            let new_committed_field = merge_bool_field(uncommitted, committed, field_dir)?;

            committed.bool_index.insert(*field_id, new_committed_field);
        }

        let strings_dir = data_dir.join("strings");
        for (field_id, field) in &uncommitted.string_index {
            let committed = committed.string_index.get(field_id);

            let field_dir = strings_dir.join(format!("field-{}", field_id.0));

            let new_committed_field = merge_string_field(uncommitted, committed, field_dir)?;


            committed.string_index.insert(*field_id, new_committed_field);
        }

        Ok(())

             */

        /*
        let m = COMMIT_METRIC.create(CommitLabels {
            side: "read",
            collection: self.id.0.to_string(),
            index_type: "string",
        });
        let string_dir = data_dir.join("strings");
        self.string_index
            .commit(string_dir)
            .await
            .context("Cannot commit string index")?;
        drop(m);

        let m = COMMIT_METRIC.create(CommitLabels {
            side: "read",
            collection: self.id.0.to_string(),
            index_type: "number",
        });
        let number_dir = data_dir.join("numbers");
        self.number_index
            .commit(number_dir)
            .await
            .context("Cannot commit number index")?;
        drop(m);

        let m = COMMIT_METRIC.create(CommitLabels {
            side: "read",
            collection: self.id.0.to_string(),
            index_type: "vector",
        });
        let vector_dir = data_dir.join("vectors");
        self.vector_index
            .commit(vector_dir)
            .context("Cannot commit vector index")?;
        drop(m);

        let m = COMMIT_METRIC.create(CommitLabels {
            side: "read",
            collection: self.id.0.to_string(),
            index_type: "bool",
        });
        let bool_dir = data_dir.join("bools");
        self.bool_index
            .commit(bool_dir)
            .await
            .context("Cannot commit bool index")?;
        drop(m);

        trace!("Committing collection info");
        let dump = dump::CollectionInfo::V1(dump::CollectionInfoV1 {
            id: self.id.clone(),
            fields: self
                .fields
                .iter()
                .map(|v| {
                    let (field_name, (field_id, typed_field)) = v.pair();

                    let typed_field = match typed_field {
                        TypedField::Bool => dump::TypedField::Bool,
                        TypedField::Number => dump::TypedField::Number,
                        TypedField::Text(language) => dump::TypedField::Text(*language),
                        TypedField::Embedding(embedding) => {
                            dump::TypedField::Embedding(dump::EmbeddingTypedField {
                                model: OramaModelSerializable(embedding.model),
                                document_fields: embedding.document_fields.clone(),
                            })
                        }
                    };

                    (field_name.clone(), (*field_id, typed_field))
                })
                .collect(),
            used_models: self
                .fields_per_model
                .iter()
                .map(|v| {
                    let (model, field_ids) = v.pair();
                    (OramaModelSerializable(*model), field_ids.clone())
                })
                .collect(),
        });
        let coll_desc_file_path = data_dir.join("info.json");
        create_or_overwrite(coll_desc_file_path, &dump)
            .await
            .context("Cannot create info.json file")?;
        trace!("Collection info committed");
        */

        Ok(())
    }

    pub fn increment_document_count(&self) {
        self.document_count.fetch_add(1, Ordering::Relaxed);
    }

    pub async fn update(
        &self,
        offset: Offset,
        collection_operation: CollectionWriteOperation,
    ) -> Result<()> {
        // We don't allow insertion during the commit
        let commit_insert_mutex_lock = self.commit_insert_mutex.lock().await;

        match collection_operation {
            CollectionWriteOperation::InsertDocument { .. } => {
                unreachable!("InsertDocument is not managed by the collection");
            }
            CollectionWriteOperation::CreateField {
                field_id,
                field_name,
                field: typed_field,
            } => {
                trace!(collection_id=?self.id, ?field_id, ?field_name, ?typed_field, "Creating field");

                let typed_field = match typed_field {
                    dto::TypedField::Embedding(model) => TypedField::Embedding(model.model),
                    dto::TypedField::Text(locale) => TypedField::Text(locale),
                    dto::TypedField::Number => TypedField::Number,
                    dto::TypedField::Bool => TypedField::Bool,
                };

                self.fields
                    .insert(field_name.clone(), (field_id, typed_field.clone()));

                self.offset_storage.set_offset(offset);

                match typed_field {
                    TypedField::Embedding(model) => {
                        self.fields_per_model
                            .entry(model)
                            .or_default()
                            .push(field_id);
                    }
                    TypedField::Text(language) => {
                        let locale = language.into();
                        let text_parser = self.nlp_service.get(locale);
                        self.text_parser_per_field
                            .insert(field_id, (locale, text_parser));
                    }
                    _ => {}
                }

                trace!("Field created");
            }
            CollectionWriteOperation::Index(doc_id, field_id, field_op) => {
                trace!(collection_id=?self.id, ?doc_id, ?field_id, ?field_op, "Indexing a new value");

                self.offset_storage.set_offset(offset);

                self.uncommitted_collection
                    .write()
                    .await
                    .insert(field_id, doc_id, field_op)?;

                trace!("Value indexed");
            }
        };

        drop(commit_insert_mutex_lock);

        Ok(())
    }

    #[instrument(skip(self, search_params), level="debug", fields(coll_id = ?self.id))]
    pub async fn search(
        &self,
        search_params: SearchParams,
    ) -> Result<HashMap<DocumentId, f32>, anyhow::Error> {
        info!(search_params = ?search_params, "Searching");
        let metric = SEARCH_METRIC.create(SearchLabels {
            collection: self.id.0.to_string(),
        });
        let SearchParams {
            mode,
            properties,
            boost,
            limit,
            where_filter,
            ..
        } = search_params;

        let filtered_doc_ids = self.calculate_filtered_doc_ids(where_filter).await?;
        let boost = self.calculate_boost(boost);

        let token_scores = match mode {
            SearchMode::Default(search_params) | SearchMode::FullText(search_params) => {
                let properties = self.calculate_string_properties(properties)?;
                self.search_full_text(
                    &search_params.term,
                    properties,
                    boost,
                    filtered_doc_ids.as_ref(),
                )
                .await?
            }
            SearchMode::Vector(search_params) => {
                self.search_vector(&search_params.term, filtered_doc_ids.as_ref(), &limit)
                    .await?
            }
            SearchMode::Hybrid(search_params) => {
                let properties = self.calculate_string_properties(properties)?;

                let (vector, fulltext) = join!(
                    self.search_vector(&search_params.term, filtered_doc_ids.as_ref(), &limit),
                    self.search_full_text(
                        &search_params.term,
                        properties,
                        boost,
                        filtered_doc_ids.as_ref()
                    )
                );
                let vector = vector?;
                let fulltext = fulltext?;

                // min-max normalization
                let max = vector.values().copied().fold(0.0, f32::max);
                let max = max.max(fulltext.values().copied().fold(0.0, f32::max));
                let min = vector.values().copied().fold(0.0, f32::min);
                let min = min.min(fulltext.values().copied().fold(0.0, f32::min));

                let vector: HashMap<_, _> = vector
                    .into_iter()
                    .map(|(k, v)| (k, (v - min) / (max - min)))
                    .collect();

                let mut fulltext: HashMap<_, _> = fulltext
                    .into_iter()
                    .map(|(k, v)| (k, (v - min) / (max - min)))
                    .collect();

                for (k, v) in vector {
                    let e = fulltext.entry(k).or_default();
                    *e += v;
                }
                fulltext
            }
        };

        info!("token_scores len: {:?}", token_scores.len());
        debug!("token_scores: {:?}", token_scores);

        drop(metric);

        Ok(token_scores)
    }

    pub fn count_documents(&self) -> u64 {
        self.document_count.load(Ordering::Relaxed)
    }

    fn calculate_boost(&self, boost: HashMap<String, f32>) -> HashMap<FieldId, f32> {
        boost
            .into_iter()
            .filter_map(|(field_name, boost)| {
                let field_id = self.get_field_id(field_name).ok()?;
                Some((field_id, boost))
            })
            .collect()
    }

    async fn calculate_filtered_doc_ids(
        &self,
        where_filter: HashMap<String, Filter>,
    ) -> Result<Option<HashSet<DocumentId>>> {
        if where_filter.is_empty() {
            return Ok(None);
        }

        info!(
            "where_filter: {:?} {:?}",
            where_filter, self.uncommitted_collection
        );

        let metric = SEARCH_FILTER_METRIC.create(SearchFilterLabels {
            collection: self.id.0.to_string(),
        });

        let filters: Result<Vec<_>> = where_filter
            .into_iter()
            .map(|(field_name, value)| {
                // This error should be typed.
                // We could return a formatted message to http
                // so, the user can understand what is wrong
                // TODO: do it
                self.get_field_id_with_type(&field_name)
                    .with_context(|| format!("Cannot filter by \"{}\": unknown field", &field_name))
                    .map(|(field_id, field_type)| (field_name, field_id, field_type, value))
            })
            .collect();
        let mut filters = filters?;
        let (field_name, field_id, field_type, filter) = filters
            .pop()
            .expect("filters is not empty here. it is already checked");

        info!(
            "Filtering on field {:?}({:?}): {:?}",
            field_name, field_type, filter
        );

        let mut doc_ids =
            get_filtered_document(&self, field_name, field_id, &field_type, filter).await?;
        for (field_name, field_id, field_type, filter) in filters {
            let doc_ids_for_field =
                get_filtered_document(&self, field_name, field_id, &field_type, filter).await?;
            doc_ids = doc_ids.intersection(&doc_ids_for_field).copied().collect();
        }

        drop(metric);

        SEARCH_FILTER_HISTOGRAM
            .create(SearchFilterLabels {
                collection: self.id.0.to_string(),
            })
            .record_usize(doc_ids.len());
        info!("Matching doc from filters: {:?}", doc_ids.len());

        Ok(Some(doc_ids))
    }

    fn calculate_string_properties(&self, properties: Properties) -> Result<Vec<FieldId>> {
        let properties: Vec<_> = match properties {
            Properties::Specified(properties) => {
                let mut r = Vec::with_capacity(properties.len());
                for field_name in properties {
                    let field = self.fields.get(&field_name);
                    let field = match field {
                        None => return Err(anyhow!("Unknown field name {}", field_name)),
                        Some(field) => field,
                    };
                    if !matches!(field.1, TypedField::Text(_)) {
                        return Err(anyhow!("Cannot search on non-string field {}", field_name));
                    }
                    r.push(field.0);
                }
                r
            }
            Properties::None | Properties::Star => {
                let mut r = Vec::with_capacity(self.fields.len());
                for field in &self.fields {
                    if !matches!(field.1, TypedField::Text(_)) {
                        continue;
                    }
                    r.push(field.0);
                }
                r
            }
        };

        Ok(properties)
    }

    async fn search_full_text(
        &self,
        term: &str,
        properties: Vec<FieldId>,
        boost: HashMap<FieldId, f32>,
        filtered_doc_ids: Option<&HashSet<DocumentId>>,
    ) -> Result<HashMap<DocumentId, f32>> {
        let mut scorer: BM25Scorer<DocumentId> = BM25Scorer::new();

        let mut tokens_cache: HashMap<Locale, Vec<String>> = Default::default();

        let committed_lock = self.committed_collection.read().await;
        let uncommitted_lock = self.uncommitted_collection.read().await;

        for field_id in properties {
            info!(?field_id, "Searching on field");
            let text_parser = self.text_parser_per_field.get(&field_id);
            let (locale, text_parser) = match text_parser.as_ref() {
                None => return Err(anyhow!("No text parser for this field")),
                Some(text_parser) => (text_parser.0, text_parser.1.clone()),
            };

            let tokens = tokens_cache
                .entry(locale)
                .or_insert_with(|| text_parser.tokenize(term));

            let committed_global_info = committed_lock.global_info(&field_id);
            let uncommitted_global_info = uncommitted_lock.global_info(&field_id);
            let global_info = committed_global_info + uncommitted_global_info;

            committed_lock.fulltext_search(
                tokens,
                vec![field_id],
                &boost,
                filtered_doc_ids,
                &mut scorer,
                &global_info,
            )?;
            uncommitted_lock.fulltext_search(
                tokens,
                vec![field_id],
                &boost,
                filtered_doc_ids,
                &mut scorer,
                &global_info,
            )?;
        }

        Ok(scorer.get_scores())
    }

    async fn search_vector(
        &self,
        term: &str,
        filtered_doc_ids: Option<&HashSet<DocumentId>>,
        limit: &Limit,
    ) -> Result<HashMap<DocumentId, f32>> {
        let mut output: HashMap<DocumentId, f32> = HashMap::new();

        let committed_lock = self.committed_collection.read().await;
        let uncommitted_lock = self.uncommitted_collection.read().await;

        for e in &self.fields_per_model {
            let model = e.key();
            let fields = e.value();

            let e = self
                .ai_service
                .embed_query(*model, vec![&term.to_string()])
                .await?;

            for k in e {
                committed_lock.vector_search(
                    &k,
                    &fields,
                    filtered_doc_ids,
                    limit.0,
                    &mut output,
                )?;
                uncommitted_lock.vector_search(&k, &fields, filtered_doc_ids, &mut output)?;
            }
        }

        Ok(output)
    }

    pub async fn calculate_facets(
        &self,
        token_scores: &HashMap<DocumentId, f32>,
        facets: HashMap<String, FacetDefinition>,
    ) -> Result<Option<HashMap<String, FacetResult>>> {
        Ok(None)
        /*
        if facets.is_empty() {
            Ok(None)
        } else {
            info!("Computing facets on {:?}", facets.keys());

            let mut res_facets: HashMap<String, FacetResult> = HashMap::new();
            for (field_name, facet) in facets {
                let field_id = self.get_field_id(field_name.clone())?;

                // This calculation is not efficient
                // we have the doc_ids that matches:
                // - filters
                // - search
                // We should use them to calculate the facets
                // Instead here we are building an hashset and
                // iter again on it to filter the doc_ids.
                // We could create a dedicated method in the indexes that
                // accepts the matching doc_ids + facet definition and returns the count
                // TODO: do it
                match facet {
                    FacetDefinition::Number(facet) => {
                        let mut values = HashMap::new();

                        for range in facet.ranges {
                            let facet: HashSet<_> = self
                                .number_index
                                .filter(field_id, NumberFilter::Between((range.from, range.to)))
                                .await?
                                .into_iter()
                                .filter(|doc_id| token_scores.contains_key(doc_id))
                                .collect();

                            values.insert(format!("{}-{}", range.from, range.to), facet.len());
                        }

                        res_facets.insert(
                            field_name,
                            FacetResult {
                                count: values.len(),
                                values,
                            },
                        );
                    }
                    FacetDefinition::Bool(facets) => {
                        let mut values = HashMap::new();

                        if facets.r#true {
                            let true_facet: HashSet<DocumentId> = self
                                .bool_index
                                .filter(field_id, true)
                                .await?
                                .into_iter()
                                .filter(|doc_id| token_scores.contains_key(doc_id))
                                .collect();
                            values.insert("true".to_string(), true_facet.len());
                        }
                        if facets.r#false {
                            let false_facet: HashSet<DocumentId> = self
                                .bool_index
                                .filter(field_id, false)
                                .await?
                                .into_iter()
                                .filter(|doc_id| token_scores.contains_key(doc_id))
                                .collect();
                            values.insert("false".to_string(), false_facet.len());
                        }

                        res_facets.insert(
                            field_name,
                            FacetResult {
                                count: values.len(),
                                values,
                            },
                        );
                    }
                }
            }
            Ok(Some(res_facets))
        }
        */
    }
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Committed {
    pub epoch: u64,
}

mod dump {
    use serde::{Deserialize, Serialize};

    use crate::{
        collection_manager::{
            dto::{DocumentFields, FieldId},
            sides::OramaModelSerializable,
        },
        nlp::locales::Locale,
        types::CollectionId,
    };

    use super::committed;

    #[derive(Debug, Serialize, Deserialize)]
    #[serde(tag = "version")]
    pub enum CollectionInfo {
        #[serde(rename = "1")]
        V1(CollectionInfoV1),
    }

    #[derive(Debug, Serialize, Deserialize)]
    pub struct CollectionInfoV1 {
        pub id: CollectionId,
        pub fields: Vec<(String, (FieldId, TypedField))>,
        pub used_models: Vec<(OramaModelSerializable, Vec<FieldId>)>,
        pub number_field_infos: Vec<(FieldId, committed::fields::NumberFieldInfo)>,
        pub string_field_infos: Vec<(FieldId, committed::fields::StringFieldInfo)>,
        pub bool_field_infos: Vec<(FieldId, committed::fields::BoolFieldInfo)>,
        pub vector_field_infos: Vec<(FieldId, committed::fields::VectorFieldInfo)>,
    }

    #[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
    pub struct EmbeddingTypedField {
        pub model: OramaModelSerializable,
    }

    #[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
    pub enum TypedField {
        Text(Locale),
        Embedding(EmbeddingTypedField),
        Number,
        Bool,
    }
}

async fn get_bool_filtered_document(
    reader: &CollectionReader,
    field_id: FieldId,
    filter_bool: bool,
) -> Result<HashSet<DocumentId>> {
    let lock = reader.uncommitted_collection.read().await;
    let uncommitted_output = lock.calculate_bool_filter(field_id, filter_bool)?;

    let lock = reader.committed_collection.read().await;
    let committed_output = lock.calculate_bool_filter(field_id, filter_bool)?;

    let result = match (uncommitted_output, committed_output) {
        (Some(uncommitted_output), Some(committed_output)) => {
            committed_output.chain(uncommitted_output).collect()
        }
        (Some(uncommitted_output), None) => uncommitted_output.collect(),
        (None, Some(committed_output)) => committed_output.collect(),
        // This case probable means the field is not a number indexed
        (None, None) => HashSet::new(),
    };

    Ok(result)
}

async fn get_number_filtered_document(
    reader: &CollectionReader,
    field_id: FieldId,
    filter_number: NumberFilter,
) -> Result<HashSet<DocumentId>> {
    let lock = reader.uncommitted_collection.read().await;
    let uncommitted_output = lock
        .calculate_number_filter(field_id, &filter_number)
        .context("Cannot calculate uncommitted filter")?;

    let lock = reader.committed_collection.read().await;
    let committed_output = lock
        .calculate_number_filter(field_id, &filter_number)
        .context("Cannot calculate committed filter")?;

    let result = match (uncommitted_output, committed_output) {
        (Some(uncommitted_output), Some(committed_output)) => {
            committed_output.chain(uncommitted_output).collect()
        }
        (Some(uncommitted_output), None) => uncommitted_output.collect(),
        (None, Some(committed_output)) => committed_output.collect(),
        // This case probable means the field is not a number indexed
        (None, None) => HashSet::new(),
    };

    Ok(result)
}

async fn get_filtered_document(
    reader: &CollectionReader,
    field_name: String,
    field_id: FieldId,
    field_type: &TypedField,
    filter: Filter,
) -> Result<HashSet<DocumentId>> {
    match (&field_type, filter) {
        (TypedField::Number, Filter::Number(filter_number)) => {
            get_number_filtered_document(reader, field_id, filter_number).await
        }
        (TypedField::Bool, Filter::Bool(filter_bool)) => {
            get_bool_filtered_document(reader, field_id, filter_bool).await
        }
        _ => {
            error!(
                "Filter on field {:?}({:?}) not supported",
                field_name, field_type
            );
            anyhow::bail!(
                "Filter on field {:?}({:?}) not supported",
                field_name,
                field_type
            )
        }
    }
}

#[derive(Debug, Clone)]
pub enum TypedField {
    Text(Locale),
    Embedding(OramaModel),
    Number,
    Bool,
}
