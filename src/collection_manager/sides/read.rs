use std::{
    any,
    cmp::Reverse,
    collections::{BinaryHeap, HashMap, HashSet},
    f32::consts::E,
    fmt::Debug,
    ops::Deref,
    pin::Pin,
    sync::{
        atomic::{AtomicU32, AtomicU64},
        Arc,
    },
};

use anyhow::{anyhow, Context, Result};
use dashmap::DashMap;
use ordered_float::NotNan;
use tokio::sync::{RwLock, RwLockReadGuard};
use tracing::{debug, error, info, instrument, warn};

use crate::{
    collection_manager::{
        collection::TokenScore,
        dto::{
            Filter, FulltextMode, SearchMode, SearchParams, SearchResult, SearchResultHit,
            TypedField,
        },
        CollectionId, FieldId,
    },
    document_storage::DocumentId,
    embeddings::EmbeddingService,
    indexes::{
        bool::BoolIndex,
        number::NumberIndex,
        string::{
            posting_storage::PostingListId, scorer::bm25::BM25Score, Posting, StringIndex,
            StringIndexValue,
        },
        vector::{VectorIndex, VectorIndexConfig},
    },
    nlp::TextParser,
    types::Document,
};

use super::{
    document_storage::DocumentStorage,
    write::{CollectionWriteOperation, GenericWriteOperation, WriteOperation},
};

pub struct CollectionsReader {
    embedding_service: Arc<EmbeddingService>,
    collections: RwLock<HashMap<CollectionId, CollectionReader>>,
    document_storage: Arc<Pin<Box<dyn DocumentStorage>>>,
    posting_id_generator: Arc<AtomicU32>,
}
impl CollectionsReader {
    pub fn new(
        embedding_service: Arc<EmbeddingService>,
        document_storage: Arc<Pin<Box<dyn DocumentStorage>>>,
    ) -> Self {
        Self {
            embedding_service,
            collections: Default::default(),
            document_storage,
            posting_id_generator: Arc::new(AtomicU32::new(0)),
        }
    }

    pub async fn update(&self, op: WriteOperation) -> Result<()> {
        match op {
            WriteOperation::Generic(GenericWriteOperation::CreateCollection { id }) => {
                info!("CreateCollection {:?}", id);
                let collection_reader = CollectionReader {
                    id: id.clone(),
                    embedding_service: self.embedding_service.clone(),

                    document_storage: Arc::clone(&self.document_storage),

                    // The unwrap here is bad even if it is safe because it never fails
                    // TODO: remove this unwrap
                    vector_index: VectorIndex::try_new(VectorIndexConfig {})
                        .context("Cannot create vector index during collection creation")?,
                    string_index: StringIndex::new(self.posting_id_generator.clone()),
                    number_index: NumberIndex::new(),
                    bool_index: BoolIndex::new(),

                    fields: Default::default(),
                };

                self.collections.write().await.insert(id, collection_reader);
            }
            WriteOperation::Collection(collection_id, coll_op) => {
                let collections = self.collections.read().await;

                let collection_reader = match collections.get(&collection_id) {
                    Some(collection_reader) => collection_reader,
                    None => {
                        error!(target: "Collection not found", ?collection_id);
                        return Err(anyhow::anyhow!("Collection not found"));
                    }
                };

                match coll_op {
                    CollectionWriteOperation::CreateField {
                        field_id,
                        field_name,
                        field,
                    } => {
                        collection_reader
                            .create_field(field_id, field_name, field)
                            .await
                            .context("Cannot create field")?;
                    }
                    CollectionWriteOperation::IndexEmbedding {
                        doc_id,
                        field_id,
                        value,
                    } => {
                        collection_reader
                            .index_embedding(doc_id, field_id, value)
                            .context("cannot index embedding")?;
                    }
                    CollectionWriteOperation::IndexString {
                        doc_id,
                        field_id,
                        terms,
                    } => {
                        collection_reader
                            .index_string(doc_id, field_id, terms)
                            .await
                            .context("cannot index string")?;
                    }
                    CollectionWriteOperation::InsertDocument { doc_id, doc } => {
                        collection_reader
                            .insert_document(doc_id, doc)
                            .await
                            .context("cannot insert document")?;
                    }
                }
            }
        };

        Ok(())
    }

    pub async fn get_collection<'s, 'coll>(
        &'s self,
        id: CollectionId,
    ) -> Option<CollectionReadLock<'coll>>
    where
        's: 'coll,
    {
        let r = self.collections.read().await;
        CollectionReadLock::try_new(r, id)
    }
}

pub struct CollectionReadLock<'guard> {
    lock: RwLockReadGuard<'guard, HashMap<CollectionId, CollectionReader>>,
    id: CollectionId,
}

impl<'guard> CollectionReadLock<'guard> {
    pub fn try_new(
        lock: RwLockReadGuard<'guard, HashMap<CollectionId, CollectionReader>>,
        id: CollectionId,
    ) -> Option<Self> {
        let guard = lock.get(&id);
        match &guard {
            Some(_) => {
                let _ = guard;
                Some(CollectionReadLock { lock, id })
            }
            None => None,
        }
    }
}

impl Deref for CollectionReadLock<'_> {
    type Target = CollectionReader;

    fn deref(&self) -> &Self::Target {
        // safety: the collection contains the id because we checked it before
        // no one can remove the collection from the map because we hold a read lock
        self.lock.get(&self.id).unwrap()
    }
}

pub struct CollectionReader {
    id: CollectionId,
    embedding_service: Arc<EmbeddingService>,

    document_storage: Arc<Pin<Box<dyn DocumentStorage>>>,

    fields: DashMap<String, (FieldId, TypedField)>,

    // indexes
    vector_index: VectorIndex,
    string_index: StringIndex,
    number_index: NumberIndex,
    bool_index: BoolIndex,
    // TODO: textparser -> vec<field_id>
}

impl CollectionReader {
    async fn create_field(
        &self,
        field_id: FieldId,
        field_name: String,
        field: TypedField,
    ) -> Result<()> {
        self.fields
            .insert(field_name.clone(), (field_id, field.clone()));

        if let TypedField::Embedding(embedding) = field {
            let orama_model = self
                .embedding_service
                .get_model(embedding.model_name)
                .await?;

            self.vector_index.add_field(field_id, orama_model)?;
        };

        Ok(())
    }

    fn index_embedding(
        &self,
        doc_id: DocumentId,
        field_id: FieldId,
        value: Vec<f32>,
    ) -> Result<()> {
        // `insert_batch` is designed to process multiple values at once
        // We are inserting only one value, and this is not good for performance
        // We should add an API to accept a single value and avoid the rebuild step
        // Instead, we could move the "rebuild" logic to the `VectorIndex`
        // TODO: do it.
        self.vector_index
            .insert_batch(vec![(doc_id, field_id, vec![value])])
    }

    #[instrument(skip(self, terms), level="debug", fields(self.id = ?self.id))]
    async fn index_string(
        &self,
        doc_id: DocumentId,
        field_id: FieldId,
        terms: HashMap<String, (u32, HashMap<(DocumentId, FieldId), Posting>)>,
    ) -> Result<()> {
        self.string_index.insert(doc_id, field_id, terms).await?;
        Ok(())
    }

    #[instrument(skip(self), level="debug", fields(self.id = ?self.id))]
    async fn insert_document(&self, doc_id: DocumentId, doc: Document) -> Result<()> {
        self.string_index.new_document_inserted().await;
        self.document_storage.add_document(doc_id, doc).await
    }

    fn get_field_id(&self, field_name: String) -> Result<FieldId> {
        let field_id = self.fields.get(&field_name);

        match field_id {
            Some(field_id) => Ok(field_id.0),
            None => Err(anyhow!("Field not found")),
        }
    }

    fn get_field_id_with_type(&self, field_name: &str) -> Result<(FieldId, TypedField)> {
        self.fields
            .get(field_name)
            .map(|v| v.clone())
            .ok_or_else(|| anyhow!("Field not found"))
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
            Ok(None)
        } else {
            let filters: Result<Vec<_>> = where_filter
                .into_iter()
                .map(|(field_name, value)| {
                    self.get_field_id_with_type(&field_name)
                        .with_context(|| format!("Unknown field \"{}\"", &field_name))
                        .map(|(field_id, field_type)| (field_name, field_id, field_type, value))
                })
                .collect();
            let mut filters = filters?;
            let last = filters.pop();

            let (field_name, field_id, field_type, filter) = match last {
                Some(v) => v,
                None => return Err(anyhow!("No filter provided")),
            };

            let mut doc_ids = match (&field_type, filter) {
                (TypedField::Number, Filter::Number(filter_number)) => {
                    self.number_index.filter(field_id, filter_number)
                }
                (TypedField::Bool, Filter::Bool(filter_bool)) => {
                    self.bool_index.filter(field_id, filter_bool)
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
                let doc_ids_ = match (&field_type, filter) {
                    (TypedField::Number, Filter::Number(filter_number)) => {
                        self.number_index.filter(field_id, filter_number)
                    }
                    (TypedField::Bool, Filter::Bool(filter_bool)) => {
                        self.bool_index.filter(field_id, filter_bool)
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
                doc_ids = doc_ids.intersection(&doc_ids_).copied().collect();
            }

            info!("Matching doc from filters: {:?}", doc_ids);

            Ok(Some(doc_ids))
        }
    }

    fn calculate_properties(&self, properties: Option<Vec<String>>) -> Result<Vec<FieldId>> {
        let properties: Result<Vec<_>> = match properties {
            Some(properties) => properties
                .into_iter()
                .map(|p| self.get_field_id(p))
                .collect(),
            None => self.fields.iter().map(|e| Ok(e.value().0)).collect(),
        };

        properties
    }

    #[instrument(skip(self), level="debug", fields(self.id = ?self.id))]
    pub async fn search<S: TryInto<SearchParams> + Debug>(
        &self,
        search_params: S,
    ) -> Result<SearchResult, anyhow::Error>
    where
        anyhow::Error: From<S::Error>,
        S::Error: std::fmt::Display,
    {
        let search_params = search_params
            .try_into()
            .map_err(|e| anyhow!("Cannot convert search params: {}", e))?;

        let SearchParams {
            mode,
            properties,
            boost,
            facets,
            limit,
            where_filter,
        } = search_params;

        let filtered_doc_ids = self.calculate_filtered_doc_ids(where_filter)?;
        let boost = self.calculate_boost(boost);
        let properties = self.calculate_properties(properties)?;

        let tokens = match &mode {
            SearchMode::FullText(FulltextMode { term }) => {
                let text_parser = TextParser::from_language(crate::nlp::locales::Locale::EN);
                text_parser.tokenize(term)
            }
            _ => unimplemented!(""),
        };

        let token_scores = self
            .string_index
            .search(
                tokens,
                // This option is not required.
                // It was introduced because for test purposes we
                // could avoid to pass every properties
                // Anyway the production code should always pass the properties
                // So we could avoid this option
                // TODO: remove this option
                Some(properties),
                boost,
                BM25Score::default(),
                filtered_doc_ids.as_ref(),
            )
            .await?;

        info!("token_scores len: {:?}", token_scores.len());

        debug!("token_scores: {:?}", token_scores);

        let count = token_scores.len();

        let top_results = top_n(token_scores, limit.0);

        let docs = self
            .document_storage
            .get_documents_by_ids(top_results.iter().map(|m| m.document_id).collect())
            .await?;

        let hits: Vec<_> = top_results
            .into_iter()
            .zip(docs)
            .map(|(token_score, document)| {
                let id = document
                    .as_ref()
                    .and_then(|d| d.get("id"))
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string())
                    .unwrap_or_default();
                SearchResultHit {
                    id,
                    score: token_score.score,
                    document,
                }
            })
            .collect();

        Ok(SearchResult {
            count,
            hits,
            facets: None,
        })
    }
}

fn top_n(map: HashMap<DocumentId, f32>, n: usize) -> Vec<TokenScore> {
    // A min-heap of size `n` to keep track of the top N elements
    let mut heap: BinaryHeap<Reverse<(NotNan<f32>, DocumentId)>> = BinaryHeap::with_capacity(n);

    for (key, value) in map {
        // Insert into the heap if it's not full, or replace the smallest element if the current one is larger
        if heap.len() < n {
            heap.push(Reverse((NotNan::new(value).unwrap(), key)));
        } else if let Some(Reverse((min_value, _))) = heap.peek() {
            if value > *min_value.as_ref() {
                heap.pop();
                heap.push(Reverse((NotNan::new(value).unwrap(), key)));
            }
        }
    }

    // Collect results into a sorted Vec (optional sorting based on descending values)
    let result: Vec<TokenScore> = heap
        .into_sorted_vec()
        .into_iter()
        .map(|Reverse((value, key))| TokenScore {
            document_id: key,
            score: value.into_inner(),
        })
        .collect();

    result
}
