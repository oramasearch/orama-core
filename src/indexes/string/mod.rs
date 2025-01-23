use std::{
    collections::{HashMap, HashSet},
    ops::{Add, AddAssign},
    path::PathBuf,
};

use anyhow::{anyhow, Context, Result};

pub use committed::CommittedStringFieldIndex;
use dashmap::DashMap;

use posting_storage::PostingListId;
use serde::{Deserialize, Serialize};
use tracing::{debug, info, instrument, warn};
pub use uncommitted::UncommittedStringFieldIndex;

use crate::{
    collection_manager::{
        dto::FieldId,
        sides::{InsertStringTerms, Offset},
    },
    field_id_hashmap::FieldIdHashMap,
    file_utils::BufferedFile,
    types::DocumentId,
};

mod committed;
mod merger;

#[cfg(any(test, feature = "benchmarking"))]
pub mod document_lengths;
#[cfg(not(any(test, feature = "benchmarking")))]
mod document_lengths;

#[cfg(any(test, feature = "benchmarking"))]
pub mod posting_storage;
#[cfg(not(any(test, feature = "benchmarking")))]
mod posting_storage;

mod scorer;
mod uncommitted;

pub use scorer::BM25Scorer;

pub type DocumentBatch = HashMap<DocumentId, Vec<(FieldId, Vec<(String, Vec<String>)>)>>;

#[derive(Debug, Clone)]
pub struct StringIndexValue {
    pub posting_list_id: PostingListId,
    pub term_frequency: usize,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct GlobalInfo {
    pub total_documents: usize,
    pub total_document_length: usize,
}
impl AddAssign for GlobalInfo {
    fn add_assign(&mut self, rhs: Self) {
        self.total_documents += rhs.total_documents;
        self.total_document_length += rhs.total_document_length;
    }
}
impl Add for GlobalInfo {
    type Output = Self;

    fn add(self, rhs: Self) -> Self::Output {
        Self {
            total_documents: self.total_documents + rhs.total_documents,
            total_document_length: self.total_document_length + rhs.total_document_length,
        }
    }
}

pub struct StringIndexConfig {}

#[derive(Debug)]
pub struct StringIndex {
    uncommitted: DashMap<FieldId, UncommittedStringFieldIndex>,
    committed: DashMap<FieldId, CommittedStringFieldIndex>,
}

impl StringIndex {
    pub fn new(_: StringIndexConfig) -> Self {
        StringIndex {
            uncommitted: DashMap::new(),
            committed: DashMap::new(),
        }
    }

    pub fn add_field(&self, offset: Offset, field_id: FieldId) {
        self.uncommitted
            .insert(field_id, UncommittedStringFieldIndex::new(offset));
    }

    #[allow(clippy::type_complexity)]
    pub fn insert(
        &self,
        offset: Offset,
        doc_id: DocumentId,
        field_id: FieldId,
        field_length: u16,
        terms: InsertStringTerms,
    ) -> Result<()> {
        let uncommitted = match self.uncommitted.get(&field_id) {
            Some(uncommitted) => uncommitted,
            None => {
                return Err(anyhow!(
                    "Field {:?} is not present in uncommitted",
                    field_id
                ));
            }
        };

        uncommitted.insert(offset, doc_id, field_length, terms)?;

        Ok(())
    }

    #[instrument(skip(self, new_path))]
    pub fn commit(&self, new_path: PathBuf) -> Result<()> {
        let all_fields = self
            .uncommitted
            .iter()
            .map(|e| *e.key())
            .chain(self.committed.iter().map(|e| *e.key()))
            .collect::<HashSet<_>>();

        info!("Dumping all fields {:?} at {:?}", all_fields, new_path);

        std::fs::create_dir_all(new_path.clone())
            .with_context(|| format!("Cannot create directory at {:?}", new_path))?;

        let mut string_index_info = StringIndexInfoV1 {
            fields: self
                .committed
                .iter()
                .map(|e| {
                    let k = *e.key();
                    let v = e.value();

                    (k, v.get_info())
                })
                .collect(),
        };

        for field_id in all_fields {
            let uncommitted = match self.uncommitted.get(&field_id) {
                Some(uncommitted) => uncommitted,
                None => {
                    warn!("Field {:?} is not present in uncommitted", field_id);
                    continue;
                }
            };
            let committed = self.committed.get(&field_id);

            let data_to_commit = uncommitted.take().context("Cannot take data to commit")?;

            if data_to_commit.is_empty() {
                info!(
                    "Everything is already committed for string field {:?}. Skip dumping",
                    field_id
                );
                continue;
            }

            let offset = data_to_commit.get_offset();
            let field_new_path = new_path
                .join(format!("field-{}", field_id.0))
                .join(format!("offset-{}", offset.0));

            std::fs::create_dir_all(field_new_path.clone())
                .with_context(|| format!("Cannot create directory at {:?}", field_new_path))?;

            let fst_path = field_new_path.join("fst.bin");
            let posting_path = field_new_path.join("posting.bin");
            let document_length_path = field_new_path.join("dl.bin");
            let global_info_path = field_new_path.join("global_info.bin");
            let posting_id_path = field_new_path.join("posting_id.bin");

            match committed {
                Some(committed) => {
                    info!("Merging field_id: {:?}", field_id);

                    merger::merge(
                        data_to_commit,
                        &committed,
                        document_length_path.clone(),
                        fst_path.clone(),
                        posting_path.clone(),
                        global_info_path.clone(),
                        posting_id_path.clone(),
                    )
                    .with_context(|| format!("Cannot merge field_id: {:?}", field_id))?;
                }
                None => {
                    info!("Dumping new field_id: {:?}", field_id);

                    merger::create(
                        data_to_commit,
                        document_length_path.clone(),
                        fst_path.clone(),
                        posting_path.clone(),
                        global_info_path.clone(),
                        posting_id_path.clone(),
                    )
                    .with_context(|| format!("Cannot create field_id: {:?}", field_id))?;
                }
            };

            let string_index_field_info = StringIndexFieldInfo {
                field_id,
                fst_path,
                document_length_path,
                posting_path,
                global_info_path,
                posting_id_path,
                offset,
            };

            let committed = CommittedStringFieldIndex::try_new(string_index_field_info.clone())
                .context("Cannot reload committed field")?;

            // Replace old field or insert new one
            self.committed.insert(field_id, committed);

            string_index_info
                .fields
                .insert(field_id, string_index_field_info);
        }

        let string_index_info = StringIndexInfo::V1(string_index_info);

        let field_file = new_path.join("info.json");
        BufferedFile::create_or_overwrite(field_file)
            .context("Cannot create info.json file")?
            .write_json_data(&string_index_info)
            .context("Cannot serialize collection info")?;

        Ok(())
    }

    pub fn load(&mut self, path: PathBuf) -> Result<()> {
        if !self.committed.is_empty() {
            return Err(anyhow!("Cannot load string index if it is not empty"));
        }

        info!("Loading string index from {:?}", path);
        match std::fs::exists(path.clone()) {
            Err(e) => {
                return Err(anyhow!("Cannot check if path exists: {:?}", e));
            }
            Ok(false) => {
                info!("Path {:?} does not exist. Skip loading", path);
                return Ok(());
            }
            Ok(true) => {}
        };

        let field_file = path.join("info.json");
        let dump: StringIndexInfo = BufferedFile::open(field_file.clone())
            .context("Cannot open info.json file")?
            .read_json_data()
            .context("Cannot deserialize info.json")?;
        let StringIndexInfo::V1(dump) = dump;

        debug!("Dump: {:?}", dump);

        for (field_id, field_dump) in dump.fields.into_inner() {
            let committed = CommittedStringFieldIndex::try_new(field_dump.clone())
                .context("Cannot reload committed field")?;

            self.committed.insert(field_id, committed);
            self.uncommitted.insert(
                field_id,
                UncommittedStringFieldIndex::new(field_dump.offset),
            );
        }

        Ok(())
    }

    pub async fn search(
        &self,
        tokens: &[String],
        search_on: Option<&[FieldId]>,
        boost: &HashMap<FieldId, f32>,
        scorer: &mut BM25Scorer<DocumentId>,
        filtered_doc_ids: Option<&HashSet<DocumentId>>,
    ) -> Result<()> {
        let search_on: HashSet<_> = if let Some(v) = search_on {
            v.iter().copied().collect()
        } else {
            self.uncommitted
                .iter()
                .map(|e| *e.key())
                .chain(self.committed.iter().map(|e| *e.key()))
                .collect()
        };

        for field_id in search_on {
            let boost = boost.get(&field_id).copied().unwrap_or(1.0);

            let uncommitted = self.uncommitted.get(&field_id);
            let committed = self.committed.get(&field_id);

            let mut global_info = committed
                .as_ref()
                .map(|c| c.get_global_info())
                .unwrap_or_default();

            // We share the global info between committed and uncommitted indexes
            // Anyway the postings aren't shared, so if a word is in both indexes:
            // - it will be scored twice
            // - the occurrence (stored in the posting) will be different
            // We can fix this, but the count would be hard.
            // Anyway, the impact of this is low, so we can ignore it for now
            // because, soon or later, we will merge the uncommitted index into the committed one.
            // So for now, we "/10" the boost of the uncommitted index, where "10" is an arbitrary number.
            // TODO: evaluate the impact of this and fix it if needed

            if let Some(uncommitted) = uncommitted {
                global_info += uncommitted.get_global_info();
                uncommitted.search(tokens, boost / 10.0, scorer, filtered_doc_ids, &global_info)?;
            }

            if let Some(committed) = committed {
                committed.search(tokens, boost, scorer, filtered_doc_ids, &global_info)?;
            }
        }

        Ok(())
    }

    #[cfg(any(test, feature = "benchmarking"))]
    pub fn remove_committed_field(&self, field_id: FieldId) -> Option<CommittedStringFieldIndex> {
        self.committed.remove(&field_id).map(|e| e.1)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StringIndexFieldInfo {
    field_id: FieldId,
    fst_path: PathBuf,
    document_length_path: PathBuf,
    posting_path: PathBuf,
    global_info_path: PathBuf,
    posting_id_path: PathBuf,
    offset: Offset,
}
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "version")]
enum StringIndexInfo {
    #[serde(rename = "1")]
    V1(StringIndexInfoV1),
}

#[derive(Debug, Serialize, Deserialize)]
struct StringIndexInfoV1 {
    fields: FieldIdHashMap<StringIndexFieldInfo>,
}

#[cfg(test)]
mod tests {
    use assert_approx_eq::assert_approx_eq;
    use serde_json::json;

    use crate::{
        collection_manager::sides::{Term, TermStringField},
        test_utils::{create_string_index, generate_new_path},
    };

    use super::*;

    #[tokio::test]
    async fn test_indexes_string_insert_search_commit_search() -> Result<()> {
        let _ = tracing_subscriber::fmt::try_init();

        let string_index = create_string_index(
            vec![(FieldId(0), "field".to_string())],
            vec![
                json!({
                    "field": "hello hello world",
                })
                .try_into()?,
                json!({
                    "field": "hello tom",
                })
                .try_into()?,
            ],
        )
        .await?;

        let mut scorer = BM25Scorer::new();
        string_index
            .search(
                &["hello".to_string()],
                None,
                &Default::default(),
                &mut scorer,
                None,
            )
            .await?;
        let before_output = scorer.get_scores();

        string_index.commit(generate_new_path())?;

        let mut scorer = BM25Scorer::new();
        string_index
            .search(
                &["hello".to_string()],
                None,
                &Default::default(),
                &mut scorer,
                None,
            )
            .await?;
        let after_output = scorer.get_scores();

        assert_approx_eq!(
            after_output[&DocumentId(0)] / 10.0,
            before_output[&DocumentId(0)]
        );
        assert_approx_eq!(
            after_output[&DocumentId(1)] / 10.0,
            before_output[&DocumentId(1)]
        );

        string_index.insert(
            Offset(100),
            DocumentId(2),
            FieldId(0),
            1,
            HashMap::from_iter([(
                Term("hello".to_string()),
                TermStringField { positions: vec![1] },
            )]),
        )?;
        let mut scorer = BM25Scorer::new();
        string_index
            .search(
                &["hello".to_string()],
                None,
                &Default::default(),
                &mut scorer,
                None,
            )
            .await?;
        let after_insert_output = scorer.get_scores();

        assert_eq!(after_insert_output.len(), 3);
        assert!(after_insert_output.contains_key(&DocumentId(0)));
        assert!(after_insert_output.contains_key(&DocumentId(1)));
        assert!(after_insert_output.contains_key(&DocumentId(2)));

        string_index.commit(generate_new_path())?;

        let mut scorer = BM25Scorer::new();
        string_index
            .search(
                &["hello".to_string()],
                None,
                &Default::default(),
                &mut scorer,
                None,
            )
            .await?;
        let after_insert_commit_output = scorer.get_scores();

        assert_eq!(after_insert_commit_output.len(), 3);
        assert!(after_insert_commit_output.contains_key(&DocumentId(0)));
        assert!(after_insert_commit_output.contains_key(&DocumentId(1)));
        assert!(after_insert_commit_output.contains_key(&DocumentId(2)));

        Ok(())
    }

    #[tokio::test]
    async fn test_indexes_string_commit_and_load() -> Result<()> {
        let _ = tracing_subscriber::fmt::try_init();

        let string_index = create_string_index(
            vec![(FieldId(0), "field".to_string())],
            vec![
                json!({
                    "field": "hello hello world",
                })
                .try_into()?,
                json!({
                    "field": "hello tom",
                })
                .try_into()?,
            ],
        )
        .await?;

        let mut scorer = BM25Scorer::new();
        string_index
            .search(
                &["hello".to_string()],
                None,
                &Default::default(),
                &mut scorer,
                None,
            )
            .await?;
        let before_scores = scorer.get_scores();

        let new_path = generate_new_path();
        string_index.commit(new_path.clone()).unwrap();

        let new_string_index = {
            let mut index = StringIndex::new(StringIndexConfig {});
            index.load(new_path).unwrap();

            index
        };

        let mut scorer = BM25Scorer::new();
        new_string_index
            .search(
                &["hello".to_string()],
                None,
                &Default::default(),
                &mut scorer,
                None,
            )
            .await?;
        let scores = scorer.get_scores();

        // Compare scores
        // - same order
        // - same keys
        // NB: the score is not the same, but the order has to be the same

        let mut before_scores = Vec::from_iter(before_scores);
        let mut scores = Vec::from_iter(scores);

        before_scores.sort_by(|(_, score1), (_, score2)| score1.total_cmp(score2));
        scores.sort_by(|(_, score1), (_, score2)| score1.total_cmp(score2));

        let before_scores: Vec<_> = before_scores.into_iter().map(|(d, _)| d).collect();
        let scores: Vec<_> = scores.into_iter().map(|(d, _)| d).collect();

        assert_eq!(before_scores, scores);

        Ok(())
    }
}

/*
#[cfg(test)]
mod tests {
    use futures::{future::join_all, FutureExt};

    use crate::{
        collection_manager::dto::FieldId,
        document_storage::DocumentId,
        indexes::string::{scorer::bm25::BM25Score, StringIndex},
        nlp::{locales::Locale, TextParser},
    };
    use std::{collections::HashMap, sync::Arc};

    #[tokio::test]
    async fn test_empty_search_query() {
        let id_geneator = Arc::new(Default::default());
        let string_index = StringIndex::new(id_geneator);
        let parser = TextParser::from_language(Locale::EN);

        let batch: HashMap<_, _> = vec![(
            DocumentId(1),
            vec![(FieldId(0), "This is a test document.".to_string())],
        )]
        .into_iter()
        .map(|(doc_id, fields)| {
            let fields: Vec<_> = fields
                .into_iter()
                .map(|(field_id, data)| {
                    let tokens = parser.tokenize_and_stem(&data);
                    (field_id, tokens)
                })
                .collect();
            (doc_id, fields)
        })
        .collect();

        string_index.insert_multiple(batch).await.unwrap();

        let output = string_index
            .search(vec![], None, Default::default(), BM25Score::default(), None)
            .await
            .unwrap();

        assert!(
            output.is_empty(),
            "Search results should be empty for empty query"
        );
    }

    #[tokio::test]
    async fn test_search_nonexistent_term() {
        let id_geneator = Arc::new(Default::default());
        let string_index = StringIndex::new(id_geneator);
        let parser = TextParser::from_language(Locale::EN);

        let batch: HashMap<_, _> = vec![(
            DocumentId(1),
            vec![(FieldId(0), "This is a test document.".to_string())],
        )]
        .into_iter()
        .map(|(doc_id, fields)| {
            let fields: Vec<_> = fields
                .into_iter()
                .map(|(field_id, data)| {
                    let tokens = parser.tokenize_and_stem(&data);
                    (field_id, tokens)
                })
                .collect();
            (doc_id, fields)
        })
        .collect();

        string_index.insert_multiple(batch).await.unwrap();

        let output = string_index
            .search(
                vec!["nonexistent".to_string()],
                None,
                Default::default(),
                BM25Score::default(),
                None,
            )
            .await
            .unwrap();

        assert!(
            output.is_empty(),
            "Search results should be empty for non-existent term"
        );
    }

    #[tokio::test]
    async fn test_insert_empty_document() {
        let id_geneator = Arc::new(Default::default());
        let string_index = StringIndex::new(id_geneator);
        let parser = TextParser::from_language(Locale::EN);

        let batch: HashMap<_, _> = vec![(DocumentId(1), vec![(FieldId(0), "".to_string())])]
            .into_iter()
            .map(|(doc_id, fields)| {
                let fields: Vec<_> = fields
                    .into_iter()
                    .map(|(field_id, data)| {
                        let tokens = parser.tokenize_and_stem(&data);
                        (field_id, tokens)
                    })
                    .collect();
                (doc_id, fields)
            })
            .collect();

        string_index.insert_multiple(batch).await.unwrap();

        let output = string_index
            .search(
                vec!["test".to_string()],
                None,
                Default::default(),
                BM25Score::default(),
                None,
            )
            .await
            .unwrap();

        assert!(
            output.is_empty(),
            "Search results should be empty when only empty documents are indexed"
        );
    }

    #[tokio::test]
    async fn test_search_with_field_filter() {
        let id_geneator = Arc::new(Default::default());
        let string_index = StringIndex::new(id_geneator);
        let parser = TextParser::from_language(Locale::EN);

        let batch: HashMap<_, _> = vec![(
            DocumentId(1),
            vec![
                (FieldId(0), "This is a test in field zero.".to_string()),
                (FieldId(1), "Another test in field one.".to_string()),
            ],
        )]
        .into_iter()
        .map(|(doc_id, fields)| {
            let fields: Vec<_> = fields
                .into_iter()
                .map(|(field_id, data)| {
                    let tokens = parser.tokenize_and_stem(&data);
                    (field_id, tokens)
                })
                .collect();
            (doc_id, fields)
        })
        .collect();

        string_index.insert_multiple(batch).await.unwrap();

        let output = string_index
            .search(
                vec!["test".to_string()],
                Some(vec![FieldId(0)]),
                Default::default(),
                BM25Score::default(),
                None,
            )
            .await
            .unwrap();

        assert_eq!(
            output.len(),
            1,
            "Should find the document when searching in FieldId(0)"
        );

        let output = string_index
            .search(
                vec!["test".to_string()],
                Some(vec![FieldId(1)]),
                Default::default(),
                BM25Score::default(),
                None,
            )
            .await
            .unwrap();

        assert_eq!(
            output.len(),
            1,
            "Should find the document when searching in FieldId(1)"
        );

        let output = string_index
            .search(
                vec!["test".to_string()],
                Some(vec![FieldId(2)]),
                Default::default(),
                BM25Score::default(),
                None,
            )
            .await
            .unwrap();

        assert!(
            output.is_empty(),
            "Should not find any documents when searching in non-existent FieldId"
        );
    }

    #[tokio::test]
    async fn test_search_with_boosts() {
        let id_geneator = Arc::new(Default::default());
        let string_index = StringIndex::new(id_geneator);
        let parser = TextParser::from_language(Locale::EN);

        let batch: HashMap<_, _> = vec![
            (
                DocumentId(1),
                vec![(FieldId(0), "Important content in field zero.".to_string())],
            ),
            (
                DocumentId(2),
                vec![(
                    FieldId(1),
                    "Less important content in field one.".to_string(),
                )],
            ),
        ]
        .into_iter()
        .map(|(doc_id, fields)| {
            let fields: Vec<_> = fields
                .into_iter()
                .map(|(field_id, data)| {
                    let tokens = parser.tokenize_and_stem(&data);
                    (field_id, tokens)
                })
                .collect();
            (doc_id, fields)
        })
        .collect();

        string_index.insert_multiple(batch).await.unwrap();

        let mut boost = HashMap::new();
        boost.insert(FieldId(0), 2.0);

        let output = string_index
            .search(
                vec!["content".to_string()],
                None,
                boost,
                BM25Score::default(),
                None,
            )
            .await
            .unwrap();

        assert_eq!(output.len(), 2, "Should find both documents");

        let score_doc1 = output.get(&DocumentId(1)).unwrap();
        let score_doc2 = output.get(&DocumentId(2)).unwrap();

        assert!(
            score_doc1 > score_doc2,
            "Document with boosted field should have higher score"
        );
    }

    #[tokio::test]
    async fn test_insert_document_with_stop_words_only() {
        let id_geneator = Arc::new(Default::default());
        let string_index = StringIndex::new(id_geneator);
        let parser = TextParser::from_language(Locale::EN);

        let batch: HashMap<_, _> = vec![(
            DocumentId(1),
            vec![(FieldId(0), "the and but or".to_string())],
        )]
        .into_iter()
        .map(|(doc_id, fields)| {
            let fields: Vec<_> = fields
                .into_iter()
                .map(|(field_id, data)| {
                    let tokens = parser.tokenize_and_stem(&data);
                    (field_id, tokens)
                })
                .collect();
            (doc_id, fields)
        })
        .collect();

        string_index.insert_multiple(batch).await.unwrap();

        let output = string_index
            .search(
                vec!["the".to_string()],
                None,
                Default::default(),
                BM25Score::default(),
                None,
            )
            .await
            .unwrap();

        assert!(
            output.is_empty(),
            "Search results should be empty when only stop words are indexed"
        );
    }

    #[tokio::test]
    async fn test_search_on_empty_index() {
        let id_geneator = Arc::new(Default::default());
        let string_index = StringIndex::new(id_geneator);

        let output = string_index
            .search(
                vec!["test".to_string()],
                None,
                Default::default(),
                BM25Score::default(),
                None,
            )
            .await
            .unwrap();

        assert!(
            output.is_empty(),
            "Search results should be empty when index is empty"
        );
    }

    #[tokio::test]
    async fn test_concurrent_insertions() {
        let id_geneator = Arc::new(Default::default());
        let string_index = Arc::new(StringIndex::new(id_geneator));

        let string_index_clone1 = Arc::clone(&string_index);
        let string_index_clone2 = Arc::clone(&string_index);

        let handle1 = async {
            let parser = TextParser::from_language(Locale::EN);
            let batch: HashMap<_, _> = vec![(
                DocumentId(1),
                vec![(
                    FieldId(0),
                    "Concurrent insertion test document one.".to_string(),
                )],
            )]
            .into_iter()
            .map(|(doc_id, fields)| {
                let fields: Vec<_> = fields
                    .into_iter()
                    .map(|(field_id, data)| {
                        let tokens = parser.tokenize_and_stem(&data);
                        (field_id, tokens)
                    })
                    .collect();
                (doc_id, fields)
            })
            .collect();

            string_index_clone1.insert_multiple(batch).await.unwrap();
        }
        .boxed();

        let handle2 = async {
            let parser = TextParser::from_language(Locale::EN);
            let batch: HashMap<_, _> = vec![(
                DocumentId(2),
                vec![(
                    FieldId(0),
                    "Concurrent insertion test document two.".to_string(),
                )],
            )]
            .into_iter()
            .map(|(doc_id, fields)| {
                let fields: Vec<_> = fields
                    .into_iter()
                    .map(|(field_id, data)| {
                        let tokens = parser.tokenize_and_stem(&data);
                        (field_id, tokens)
                    })
                    .collect();
                (doc_id, fields)
            })
            .collect();

            string_index_clone2.insert_multiple(batch).await.unwrap();
        }
        .boxed();

        join_all(vec![handle1, handle2]).await;

        let parser = TextParser::from_language(Locale::EN);
        let search_tokens = parser
            .tokenize_and_stem("concurrent")
            .into_iter()
            .map(|(original, _)| original)
            .collect::<Vec<_>>();

        let output = string_index
            .search(
                search_tokens,
                None,
                Default::default(),
                BM25Score::default(),
                None,
            )
            .await
            .unwrap();

        assert_eq!(
            output.len(),
            2,
            "Should find both documents after concurrent insertions"
        );
    }

    #[tokio::test]
    async fn test_large_documents() {
        let id_geneator = Arc::new(Default::default());
        let string_index = StringIndex::new(id_geneator);
        let parser = TextParser::from_language(Locale::EN);

        let large_text = "word ".repeat(10000);

        let batch: HashMap<_, _> = vec![(DocumentId(1), vec![(FieldId(0), large_text.clone())])]
            .into_iter()
            .map(|(doc_id, fields)| {
                let fields: Vec<_> = fields
                    .into_iter()
                    .map(|(field_id, data)| {
                        let tokens = parser.tokenize_and_stem(&data);
                        (field_id, tokens)
                    })
                    .collect();
                (doc_id, fields)
            })
            .collect();

        string_index.insert_multiple(batch).await.unwrap();

        let output = string_index
            .search(
                vec!["word".to_string()],
                None,
                Default::default(),
                BM25Score::default(),
                None,
            )
            .await
            .unwrap();

        assert_eq!(
            output.len(),
            1,
            "Should find the document containing the large text"
        );
    }

    #[tokio::test]
    async fn test_high_term_frequency() {
        let id_geneator = Arc::new(Default::default());
        let string_index = StringIndex::new(id_geneator);
        let parser = TextParser::from_language(Locale::EN);

        let repeated_word = "repeat ".repeat(1000);

        let batch: HashMap<_, _> = vec![(DocumentId(1), vec![(FieldId(0), repeated_word.clone())])]
            .into_iter()
            .map(|(doc_id, fields)| {
                let fields: Vec<_> = fields
                    .into_iter()
                    .map(|(field_id, data)| {
                        let tokens = parser.tokenize_and_stem(&data);
                        (field_id, tokens)
                    })
                    .collect();
                (doc_id, fields)
            })
            .collect();

        string_index.insert_multiple(batch).await.unwrap();

        let output = string_index
            .search(
                vec!["repeat".to_string()],
                None,
                Default::default(),
                BM25Score::default(),
                None,
            )
            .await
            .unwrap();

        assert_eq!(
            output.len(),
            1,
            "Should find the document with high term frequency"
        );
    }

    #[tokio::test]
    async fn test_term_positions() {
        let id_geneator = Arc::new(Default::default());
        let string_index = StringIndex::new(id_geneator);
        let parser = TextParser::from_language(Locale::EN);

        let batch: HashMap<_, _> = vec![(
            DocumentId(1),
            vec![(
                FieldId(0),
                "quick brown fox jumps over the lazy dog".to_string(),
            )],
        )]
        .into_iter()
        .map(|(doc_id, fields)| {
            let fields: Vec<_> = fields
                .into_iter()
                .map(|(field_id, data)| {
                    let tokens = parser.tokenize_and_stem(&data);
                    (field_id, tokens)
                })
                .collect();
            (doc_id, fields)
        })
        .collect();

        string_index.insert_multiple(batch).await.unwrap();
    }

    #[tokio::test]
    async fn test_exact_phrase_match() {
        let id_geneator = Arc::new(Default::default());
        let string_index = StringIndex::new(id_geneator);
        let parser = TextParser::from_language(Locale::EN);

        let batch: HashMap<_, _> = vec![(
            DocumentId(1),
            vec![(FieldId(0), "5200 mAh battery in disguise".to_string())],
        )]
        .into_iter()
        .map(|(doc_id, fields)| {
            let fields: Vec<_> = fields
                .into_iter()
                .map(|(field_id, data)| {
                    let tokens = parser.tokenize_and_stem(&data);
                    (field_id, tokens)
                })
                .collect();
            (doc_id, fields)
        })
        .collect();

        string_index.insert_multiple(batch).await.unwrap();

        let output = string_index
            .search(
                vec![
                    "5200".to_string(),
                    "mAh".to_string(),
                    "battery".to_string(),
                    "in".to_string(),
                    "disguise".to_string(),
                ],
                Some(vec![FieldId(0)]),
                Default::default(),
                BM25Score::default(),
                None,
            )
            .await
            .unwrap();

        assert_eq!(
            output.len(),
            1,
            "Should find the document containing the exact phrase"
        );

        assert!(
            output.contains_key(&DocumentId(1)),
            "Document with ID 1 should be found"
        );
    }
}
*/
