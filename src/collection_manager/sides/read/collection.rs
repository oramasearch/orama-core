use std::{
    collections::{HashMap, HashSet},
    path::PathBuf,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
};

use anyhow::{anyhow, Context, Result};
use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use tokio::join;
use tracing::{debug, error, info, instrument};

use crate::{
    collection_manager::{
        dto::{
            FacetDefinition, FacetResult, FieldId, Filter, Limit, Properties, SearchMode,
            SearchParams, TypedField,
        },
        sides::{CollectionWriteOperation, DocumentFieldIndexOperation, Offset},
    },
    embeddings::{EmbeddingService, LoadedModel},
    file_utils::BufferedFile,
    indexes::{
        bool::BoolIndex,
        number::{NumberFilter, NumberIndex, NumberIndexConfig},
        string::{BM25Scorer, StringIndex, StringIndexConfig},
        vector::{VectorIndex, VectorIndexConfig},
    },
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
    pub(super) id: CollectionId,
    pub(super) embedding_service: Arc<EmbeddingService>,
    pub(super) nlp_service: Arc<NLPService>,

    document_count: AtomicU64,

    pub(super) fields: DashMap<String, (FieldId, TypedField)>,

    // indexes
    pub(super) vector_index: VectorIndex,
    pub(super) fields_per_model: DashMap<Arc<LoadedModel>, Vec<FieldId>>,

    pub(super) string_index: StringIndex,
    pub(super) text_parser_per_field: DashMap<FieldId, (Locale, Arc<TextParser>)>,

    pub(super) number_index: NumberIndex,
    pub(super) bool_index: BoolIndex,
    // TODO: textparser -> vec<field_id>
    offset_storage: OffsetStorage,
}

impl CollectionReader {
    pub fn try_new(
        id: CollectionId,
        embedding_service: Arc<EmbeddingService>,
        nlp_service: Arc<NLPService>,
        _: IndexesConfig,
    ) -> Result<Self> {
        let vector_index = VectorIndex::try_new(VectorIndexConfig {})
            .context("Cannot create vector index during collection creation")?;

        let string_index = StringIndex::new(StringIndexConfig {});

        let number_index = NumberIndex::try_new(NumberIndexConfig {})
            .context("Cannot create number index during collection creation")?;

        let bool_index = BoolIndex::new();

        Ok(Self {
            id,
            embedding_service,
            nlp_service,

            document_count: AtomicU64::new(0),

            vector_index,
            fields_per_model: Default::default(),

            string_index,
            text_parser_per_field: Default::default(),

            number_index,

            bool_index,

            fields: Default::default(),

            offset_storage: Default::default(),
        })
    }

    pub(super) fn get_field_id(&self, field_name: String) -> Result<FieldId> {
        let field_id = self.fields.get(&field_name);

        match field_id {
            Some(field_id) => Ok(field_id.0),
            None => Err(anyhow!("Field not found")),
        }
    }

    pub(super) fn get_field_id_with_type(&self, field_name: &str) -> Result<(FieldId, TypedField)> {
        self.fields
            .get(field_name)
            .map(|v| v.clone())
            .ok_or_else(|| anyhow!("Field not found"))
    }

    pub async fn load(&mut self, collection_data_dir: PathBuf) -> Result<()> {
        self.string_index
            .load(collection_data_dir.join("strings"))
            .context("Cannot load string index")?;
        self.number_index
            .load(collection_data_dir.join("numbers"))
            .context("Cannot load number index")?;
        // self.vector_index
        //     .load(collection_data_dir.join("vectors"))
        //     .context("Cannot load vectors index")?;

        let coll_desc_file_path = collection_data_dir.join("info.json");
        let dump: CollectionInfo = BufferedFile::open(coll_desc_file_path)
            .context("Cannot open collection file")?
            .read_json_data()
            .with_context(|| format!("Cannot deserialize collection info for {:?}", self.id))?;
        for (field_name, (field_id, field_type)) in dump.fields {
            self.fields.insert(field_name, (field_id, field_type));
        }

        for (orama_model, fields) in dump.used_models {
            let model = self
                .embedding_service
                .get_model(orama_model.clone())
                .await
                .context("Model not found")?;
            self.fields_per_model.insert(model, fields);
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

        Ok(())
    }

    pub fn commit(&self, data_dir: PathBuf) -> Result<()> {
        self.string_index
            .commit(data_dir.join("strings"))
            .context("Cannot commit string index")?;
        self.number_index
            .commit(data_dir.join("numbers"))
            .context("Cannot commit number index")?;
        // self.vector_index
        //     .commit(data_dir.join("vectors"))
        //     .context("Cannot commit vectors index")?;

        let dump = CollectionInfo {
            id: self.id.clone(),
            fields: self
                .fields
                .iter()
                .map(|v| {
                    let (field_name, (field_id, typed_field)) = v.pair();
                    (field_name.clone(), (*field_id, typed_field.clone()))
                })
                .collect(),
            used_models: self
                .fields_per_model
                .iter()
                .map(|v| {
                    let (model, field_ids) = v.pair();
                    (model.model_name(), field_ids.clone())
                })
                .collect(),
        };

        let coll_desc_file_path = data_dir.join("info.json");
        BufferedFile::create_or_overwrite(coll_desc_file_path)
            .context("Cannot create info.json file")?
            .write_json_data(&dump)
            .with_context(|| format!("Cannot serialize collection info for {:?}", self.id))?;

        Ok(())
    }

    pub fn increment_document_count(&self) {
        self.document_count.fetch_add(1, Ordering::Relaxed);
    }

    pub(super) async fn update(
        &self,
        offset: Offset,
        collection_operation: CollectionWriteOperation,
    ) -> Result<()> {
        match collection_operation {
            CollectionWriteOperation::InsertDocument { .. } => {
                unreachable!("InsertDocument is not managed by the collection");
            }
            CollectionWriteOperation::CreateField {
                field_id,
                field_name,
                field: typed_field,
            } => {
                self.fields
                    .insert(field_name.clone(), (field_id, typed_field.clone()));

                self.offset_storage.set_offset(offset);

                match typed_field {
                    TypedField::Embedding(embedding) => {
                        let loaded_model = self
                            .embedding_service
                            .get_model(embedding.model_name)
                            .await?;

                        self.vector_index
                            .add_field(field_id, loaded_model.dimensions())?;

                        self.fields_per_model
                            .entry(loaded_model)
                            .or_default()
                            .push(field_id);
                    }
                    TypedField::Text(language) => {
                        let locale = language.into();
                        let text_parser = self.nlp_service.get(locale);
                        self.text_parser_per_field
                            .insert(field_id, (locale, text_parser));
                        self.string_index.add_field(offset, field_id);
                    }
                    _ => {}
                }
            }
            CollectionWriteOperation::Index(doc_id, field_id, field_op) => {
                self.offset_storage.set_offset(offset);
                match field_op {
                    DocumentFieldIndexOperation::IndexBoolean { value } => {
                        self.bool_index.add(offset, doc_id, field_id, value)?;
                    }
                    DocumentFieldIndexOperation::IndexNumber { value } => {
                        self.number_index.add(offset, doc_id, field_id, value)?;
                    }
                    DocumentFieldIndexOperation::IndexString {
                        field_length,
                        terms,
                    } => {
                        self.string_index
                            .insert(offset, doc_id, field_id, field_length, terms)?;
                    }
                    DocumentFieldIndexOperation::IndexEmbedding { value } => {
                        // `insert_batch` is designed to process multiple values at once
                        // We are inserting only one value, and this is not good for performance
                        // We should add an API to accept a single value and avoid the rebuild step
                        // Instead, we could move the "rebuild" logic to the `VectorIndex`
                        // TODO: do it.
                        self.vector_index
                            .insert_batch(offset, vec![(doc_id, field_id, vec![value])])?;
                    }
                }
            }
        };

        Ok(())
    }

    #[instrument(skip(self), level="debug", fields(self.id = ?self.id))]
    pub(super) async fn search(
        &self,
        search_params: SearchParams,
    ) -> Result<HashMap<DocumentId, f32>, anyhow::Error> {
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

        let filtered_doc_ids = self.calculate_filtered_doc_ids(where_filter)?;
        let boost = self.calculate_boost(boost);

        let token_scores = match mode {
            SearchMode::Default(search_params) | SearchMode::FullText(search_params) => {
                let properties = self.calculate_string_properties(properties)?;
                self.search_full_text(&search_params.term, properties, boost, filtered_doc_ids)
                    .await?
            }
            SearchMode::Vector(search_params) => {
                self.search_vector(&search_params.term, filtered_doc_ids, &limit)
                    .await?
            }
            SearchMode::Hybrid(search_params) => {
                let properties = self.calculate_string_properties(properties)?;

                let (vector, fulltext) = join!(
                    self.search_vector(&search_params.term, filtered_doc_ids.clone(), &limit),
                    self.search_full_text(&search_params.term, properties, boost, filtered_doc_ids)
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

    fn calculate_filtered_doc_ids(
        &self,
        where_filter: HashMap<String, Filter>,
    ) -> Result<Option<HashSet<DocumentId>>> {
        if where_filter.is_empty() {
            return Ok(None);
        }

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

        let mut doc_ids = match (&field_type, filter) {
            (TypedField::Number, Filter::Number(filter_number)) => {
                self.number_index.filter(field_id, filter_number)?
            }
            (TypedField::Bool, Filter::Bool(filter_bool)) => {
                self.bool_index.filter(field_id, filter_bool)?
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
        };
        for (field_name, field_id, field_type, filter) in filters {
            let doc_ids_for_field = match (&field_type, filter) {
                (TypedField::Number, Filter::Number(filter_number)) => {
                    self.number_index.filter(field_id, filter_number)?
                }
                (TypedField::Bool, Filter::Bool(filter_bool)) => {
                    self.bool_index.filter(field_id, filter_bool)?
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
            };
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
        filtered_doc_ids: Option<HashSet<DocumentId>>,
    ) -> Result<HashMap<DocumentId, f32>> {
        let mut scorer: BM25Scorer<DocumentId> = BM25Scorer::new();

        let mut tokens_cache: HashMap<Locale, Vec<String>> = Default::default();

        for field_id in properties {
            info!(?field_id, "Searching on field");
            let text_parser = self.text_parser_per_field.get(&field_id);
            let (locale, text_parser) = match text_parser.as_ref() {
                None => return Err(anyhow!("No text parser for this field")),
                Some(text_parser) => (text_parser.0, &text_parser.1),
            };

            let tokens = tokens_cache
                .entry(locale)
                .or_insert_with(|| text_parser.tokenize(term));

            self.string_index
                .search(
                    tokens,
                    // This option is not required.
                    // It was introduced because for test purposes we
                    // could avoid to pass every properties
                    // Anyway the production code should always pass the properties
                    // So we could avoid this option
                    // TODO: remove this option
                    Some(&[field_id]),
                    &boost,
                    &mut scorer,
                    filtered_doc_ids.as_ref(),
                )
                .await?;
        }

        Ok(scorer.get_scores())
    }

    async fn search_vector(
        &self,
        term: &str,
        filtered_doc_ids: Option<HashSet<DocumentId>>,
        limit: &Limit,
    ) -> Result<HashMap<DocumentId, f32>> {
        let mut ret: HashMap<DocumentId, f32> = HashMap::new();

        for e in &self.fields_per_model {
            let model = e.key();
            let fields = e.value();

            let e = model.embed_query(vec![&term.to_string()]).await?;

            for k in e {
                let r = self.vector_index.search(fields, &k, limit.0)?;

                for (doc_id, score) in r {
                    if !filtered_doc_ids
                        .as_ref()
                        .map(|f| f.contains(&doc_id))
                        .unwrap_or(true)
                    {
                        continue;
                    }

                    let v = ret.entry(doc_id).or_default();
                    *v += score;
                }
            }
        }

        Ok(ret)
    }

    pub(super) fn calculate_facets(
        &self,
        token_scores: &HashMap<DocumentId, f32>,
        facets: HashMap<String, FacetDefinition>,
    ) -> Result<Option<HashMap<String, FacetResult>>> {
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
                                .filter(field_id, NumberFilter::Between((range.from, range.to)))?
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
                                .filter(field_id, true)?
                                .into_iter()
                                .filter(|doc_id| token_scores.contains_key(doc_id))
                                .collect();
                            values.insert("true".to_string(), true_facet.len());
                        }
                        if facets.r#false {
                            let false_facet: HashSet<DocumentId> = self
                                .bool_index
                                .filter(field_id, false)?
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
    }
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Committed {
    pub epoch: u64,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct CollectionInfo {
    pub id: CollectionId,
    pub fields: Vec<(String, (FieldId, TypedField))>,
    pub used_models: Vec<(String, Vec<FieldId>)>,
}
