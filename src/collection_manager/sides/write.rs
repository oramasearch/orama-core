use std::{
    collections::{hash_map::Entry, HashMap},
    fmt::Debug,
    sync::{
        atomic::{AtomicU16, AtomicU32},
        Arc,
    },
};

use anyhow::{anyhow, Context, Ok, Result};
use dashmap::DashMap;
use tokio::sync::broadcast::Sender;
use tracing::info;

use crate::{
    collection_manager::dto::{CollectionDTO, FieldId},
    document_storage::DocumentId,
    embeddings::{EmbeddingService, LoadedModel},
    indexes::string::Posting,
    nlp::{locales::Locale, TextParser},
    types::{ComplexType, Document, DocumentList, FlattenDocument, ScalarType, ValueType},
};

use super::super::{
    dto::{CreateCollectionOptionDTO, LanguageDTO, TypedField},
    CollectionId,
};

#[derive(Debug, Clone)]
pub enum GenericWriteOperation {
    CreateCollection {
        id: CollectionId,
        // Params here... but which ones?
        // TODO: add params
    },
}

pub type InsertStringTerms = HashMap<String, (u32, HashMap<(DocumentId, FieldId), Posting>)>;
type FieldsToIndex = DashMap<String, (ValueType, Arc<Box<dyn FieldIndexer>>)>;

#[derive(Debug, Clone)]
pub enum CollectionWriteOperation {
    InsertDocument {
        doc_id: DocumentId,
        doc: Document,
    },
    CreateField {
        field_id: FieldId,
        field_name: String,
        field: TypedField,
    },
    IndexString {
        doc_id: DocumentId,
        field_id: FieldId,
        terms: InsertStringTerms,
    },
    IndexEmbedding {
        doc_id: DocumentId,
        field_id: FieldId,
        value: Vec<f32>,
    },
}

#[derive(Debug, Clone)]
pub enum WriteOperation {
    Generic(GenericWriteOperation),
    Collection(CollectionId, CollectionWriteOperation),
}

pub struct CollectionsWriter {
    document_id_generator: Arc<AtomicU32>,
    sender: Sender<WriteOperation>,
    embedding_service: Arc<EmbeddingService>,
    collections: DashMap<CollectionId, CollectionWriter>,
}

impl CollectionsWriter {
    pub fn new(
        document_id_generator: Arc<AtomicU32>,
        sender: Sender<WriteOperation>,
        embedding_service: Arc<EmbeddingService>,
    ) -> CollectionsWriter {
        CollectionsWriter {
            document_id_generator,
            sender,
            embedding_service,
            collections: DashMap::new(),
        }
    }

    fn generate_document_id(&self) -> DocumentId {
        let id = self
            .document_id_generator
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        DocumentId(id)
    }

    fn get_text_parser(&self, language: Option<LanguageDTO>) -> Arc<TextParser> {
        let locale: Locale = language.unwrap_or(LanguageDTO::English).into();
        let parser = TextParser::from_language(locale);
        Arc::new(parser)
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

        let default_text_parser = self.get_text_parser(language);

        let field_id_generator = AtomicU16::new(0);

        let collection = CollectionWriter {
            id: id.clone(),
            description,
            default_language: language.unwrap_or(LanguageDTO::English),
            document_count: Default::default(),
            default_text_parser,
            fields: Default::default(),
            field_id_by_name: DashMap::new(),
            field_id_generator: Arc::new(field_id_generator),
        };

        for (field_name, field_type) in typed_fields {
            match &field_type {
                TypedField::Embedding(embedding_field) => {
                    let model = self
                        .embedding_service
                        .get_model(embedding_field.model_name.clone())
                        .await?;
                    collection.fields.insert(
                        field_name.clone(),
                        (
                            ValueType::Complex(ComplexType::Embedding),
                            Arc::new(Box::new(EmbeddingField {
                                model,
                                document_fields: embedding_field.document_fields.clone(),
                            })),
                        ),
                    );
                }
                TypedField::Text(language) => {
                    let parser = self.get_text_parser(Some(*language));
                    collection.fields.insert(
                        field_name.clone(),
                        (
                            ValueType::Scalar(ScalarType::String),
                            Arc::new(Box::new(StringField { parser })),
                        ),
                    );
                }
                TypedField::Code(_) => unimplemented!("Code field not implemented yet"),
                TypedField::Number => unimplemented!("Number field not implemented yet"),
                TypedField::Bool => unimplemented!("Bool field not implemented yet"),
            }

            let field_id = collection.get_field_id_by_name(&field_name);

            self.sender
                .send(WriteOperation::Collection(
                    id.clone(),
                    CollectionWriteOperation::CreateField {
                        field_id,
                        field_name,
                        field: field_type,
                    },
                ))
                .unwrap();
        }

        // This substitute the previous value and this is wrong.
        // We should *NOT* allow to overwrite a collection
        // We should return an error if the collection already exists
        // NB: the check of the existence of the collection and the insertion should be done atomically
        // TODO: do it.
        self.collections.insert(id.clone(), collection);

        Ok(id)
    }

    pub fn list(&self) -> Vec<CollectionDTO> {
        self.collections.iter().map(|e| {
            let coll = e.value();

            coll.as_dto()
        }).collect()
    }

    pub fn get_collection_dto(&self, collection_id: CollectionId) -> Option<CollectionDTO> {
        let collection = self.collections.get(&collection_id);
        collection.map(|c| c.as_dto())
    }

    pub async fn write(
        &self,
        collection_id: CollectionId,
        document_list: DocumentList,
    ) -> Result<()> {
        info!("Inserting batch of {} documents", document_list.len());

        let collection = self
            .collections
            .get(&collection_id)
            .ok_or_else(|| anyhow!("Collection not found"))?;

        for doc in document_list {
            let doc_id = self.generate_document_id();

            collection.document_count.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            self.sender
                .send(WriteOperation::Collection(
                    collection_id.clone(),
                    CollectionWriteOperation::InsertDocument {
                        doc_id,
                        doc: doc.clone(),
                    },
                ))
                .map_err(|e| anyhow!("Error sending document to index writer: {:?}", e))?;

            let fields_to_index = collection
                .get_fields_to_index(doc.clone(), self)
                .context("Cannot get fields to index")?;

            let flatten = doc.clone().into_flatten();
            for entry in fields_to_index.iter() {
                let (field_name, (_, field)) = entry.pair();

                let field_id = collection.get_field_id_by_name(field_name);

                let write_operations = field.get_write_operations(
                    collection_id.clone(),
                    doc_id,
                    field_name,
                    field_id,
                    &flatten,
                )?;
                for write_operation in write_operations {
                    self.sender
                        .send(write_operation)
                        .map_err(|e| anyhow!("Error sending document to index writer: {:?}", e))?;
                }
            }
        }

        Ok(())
    }
}

trait FieldIndexer: Sync + Send + Debug {
    fn get_write_operations(
        &self,
        coll_id: CollectionId,
        doc_id: DocumentId,
        field_name: &str,
        field_id: FieldId,
        doc: &FlattenDocument,
    ) -> Result<Vec<WriteOperation>>;
}

#[derive(Debug)]
pub struct StringField {
    parser: Arc<TextParser>,
}
impl FieldIndexer for StringField {
    fn get_write_operations(
        &self,
        coll_id: CollectionId,
        doc_id: DocumentId,
        field_name: &str,
        field_id: FieldId,
        doc: &FlattenDocument,
    ) -> Result<Vec<WriteOperation>> {
        let value = doc.get(field_name);

        let data = match value {
            None => return Ok(vec![]),
            Some(value) => match value.as_str() {
                None => return Ok(vec![]),
                Some(value) => self.parser.tokenize_and_stem(value),
            },
        };

        let doc_length = data.len().min(u16::MAX as usize - 1) as u16;

        let mut terms: InsertStringTerms = Default::default();
        for (position, (original, stemmeds)) in data.into_iter().enumerate() {
            // This `for` loop wants to build the `terms` hashmap
            // it is a `HashMap<String, (u32, HashMap<(DocumentId, FieldId), Posting>)>`
            // that means we:
            // term as string -> (term count, HashMap<(DocumentId, FieldId), Posting>)
            // Here we don't want to store Posting into PostingListStorage,
            // that is business of the IndexReader.
            // Instead, here we want to extrapolate data from the document.
            // The real storage leaves in the IndexReader.
            // `original` & `stemmeds` appears in the `terms` hashmap with the "same value"
            // ie: the position of the origin and stemmed term are the same.

            match terms.entry(original) {
                Entry::Occupied(mut entry) => {
                    let v = entry.get_mut();
                    v.0 += 1; // Term frequency inside the doc for this field

                    let p = v.1.entry((doc_id, field_id)).or_insert_with(|| Posting {
                        document_id: doc_id,
                        field_id,
                        positions: vec![],
                        term_frequency: 0.0,
                        doc_length,
                    });

                    p.positions.push(position);
                    p.term_frequency += 1.0;
                }
                Entry::Vacant(entry) => {
                    let p = Posting {
                        document_id: doc_id,
                        field_id,
                        positions: vec![position],
                        term_frequency: 1.0,
                        doc_length,
                    };
                    entry.insert((1, HashMap::from_iter([((doc_id, field_id), p)])));
                }
            };

            for stemmed in stemmeds {
                match terms.entry(stemmed) {
                    Entry::Occupied(mut entry) => {
                        let v = entry.get_mut();
                        v.0 += 1; // Term frequency inside the doc for this field

                        let p = v.1.entry((doc_id, field_id)).or_insert_with(|| Posting {
                            document_id: doc_id,
                            field_id,
                            positions: vec![],
                            term_frequency: 0.0,
                            doc_length,
                        });

                        p.positions.push(position);
                        p.term_frequency += 1.0;
                    }
                    Entry::Vacant(entry) => {
                        let p = Posting {
                            document_id: doc_id,
                            field_id,
                            positions: vec![position],
                            term_frequency: 1.0,
                            doc_length,
                        };
                        entry.insert((1, HashMap::from_iter([((doc_id, field_id), p)])));
                    }
                };
            }
        }

        let op = WriteOperation::Collection(
            coll_id,
            CollectionWriteOperation::IndexString {
                doc_id,
                field_id,
                terms,
            },
        );

        Ok(vec![op])
    }
}

#[derive(Debug)]
struct EmbeddingField {
    model: Arc<LoadedModel>,
    document_fields: Vec<String>,
}

impl FieldIndexer for EmbeddingField {
    fn get_write_operations(
        &self,
        coll_id: CollectionId,
        doc_id: DocumentId,
        _field_name: &str,
        field_id: FieldId,
        doc: &FlattenDocument,
    ) -> Result<Vec<WriteOperation>> {
        let input: String = self
            .document_fields
            .iter()
            .filter_map(|field_name| {
                let value = doc.get(field_name).and_then(|v| v.as_str());
                value
            })
            .collect();

        // TODO: do this better
        let mut output = self.model.embed(vec![input], None)?;
        let output = output.remove(0);

        Ok(vec![WriteOperation::Collection(
            coll_id.clone(),
            CollectionWriteOperation::IndexEmbedding {
                doc_id,
                field_id,
                value: output,
            },
        )])
    }
}

pub struct CollectionWriter {
    id: CollectionId,
    description: Option<String>,
    default_language: LanguageDTO,
    default_text_parser: Arc<TextParser>,
    fields: DashMap<String, (ValueType, Arc<Box<dyn FieldIndexer>>)>,

    document_count: AtomicU32,

    field_id_generator: Arc<AtomicU16>,
    field_id_by_name: DashMap<String, FieldId>,
}

impl CollectionWriter {
    pub fn as_dto(&self) -> CollectionDTO {
        CollectionDTO {
            id: self.id.clone(),
            description: self.description.clone(),
            document_count: self.document_count.load(std::sync::atomic::Ordering::Relaxed),
            fields: self.fields.iter().map(|e| (e.key().clone(), e.value().0.clone())).collect(),
        }
    }

    fn get_field_id_by_name(&self, name: &str) -> FieldId {
        use dashmap::Entry;

        let v = self.field_id_by_name.get(name);
        // Fast path
        if let Some(v) = v {
            return *v;
        }
        let entry = self.field_id_by_name.entry(name.to_string());
        match entry {
            // This is odd, but concurrently,
            // we can have the first `get` None and have the entry occupied
            Entry::Occupied(e) => *e.get(),
            Entry::Vacant(e) => {
                // Vacant entry locks the map, so we can safely insert the field_id
                let field_id = self
                    .field_id_generator
                    .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                let field_id = FieldId(field_id);
                e.insert(field_id);

                info!("Field created {} -> {:?}", name, field_id);

                field_id
            }
        }
    }

    fn get_fields_to_index(
        &self,
        doc: Document,
        writer: &CollectionsWriter,
    ) -> Result<FieldsToIndex> {
        let flatten = doc.clone().into_flatten();
        let schema = flatten.get_field_schema();

        for (field_name, value_type) in schema {
            if self.fields.contains_key(&field_name) {
                continue;
            }

            let field_id = self.get_field_id_by_name(&field_name);

            writer
                .sender
                .send(WriteOperation::Collection(
                    self.id.clone(),
                    CollectionWriteOperation::CreateField {
                        field_id,
                        field_name: field_name.clone(),
                        field: TypedField::Text(self.default_language),
                    },
                ))
                .context("Cannot sent creation field")?;

            match value_type {
                ValueType::Scalar(ScalarType::String) => {
                    let parser = self.default_text_parser.clone();
                    self.fields.insert(
                        field_name.clone(),
                        (value_type, Arc::new(Box::new(StringField { parser }))),
                    );
                }
                _ => unimplemented!("Field type not implemented yet"),
            }
        }

        Ok(self.fields.clone())
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

    /*
    #[tokio::test]
    async fn test_writer() {
        // let _ = tracing_subscriber::fmt::try_init();

        let (sender, mut rec) = tokio::sync::broadcast::channel(100);
        let sender = Arc::new(sender);

        let embedding_service = EmbeddingService::try_new(EmbeddingConfig {
            cache_path: std::env::temp_dir().to_string_lossy().to_string(),
            hugging_face: None,
            preload: EmbeddingPreload::Bool(false),
        })
        .await
        .unwrap();
        let embedding_service = Arc::new(embedding_service);
        let document_id_generator = Arc::new(AtomicU64::new(0));
        let writer = CollectionsWriter::new(document_id_generator, sender, embedding_service);

        let create_collection_request: CreateCollectionOptionDTO = CreateCollectionOptionDTO {
            id: "my-collection".to_string(),
            description: None,
            language: None,
            typed_fields: HashMap::from_iter([(
                "embedding".to_string(),
                TypedField::Embedding(EmbeddingTypedField {
                    model_name: OramaModel::Fastembed(OramaFastembedModel::GTESmall),
                    document_fields: vec!["title".to_string()],
                }),
            )]),
        };
        writer
            .create_collection(create_collection_request)
            .await
            .unwrap();

        let collection_id = CollectionId("my-collection".to_string());

        let coll_created = rec.try_recv().unwrap();
        assert!(matches!(
            coll_created,
            WriteOperation::Generic(GenericWriteOperation::CreateCollection { .. })
        ));

        let field_created = rec.try_recv().unwrap();
        let embedding_field_id = check_create_field(&field_created, &collection_id, "embedding");

        // No other message should be received
        let empty = rec.try_recv();
        assert!(matches!(empty, Err(TryRecvError::Empty)));

        let docs = vec![
            json!({ "title": "title of doc 1", "content": "content of doc 1" }),
            json!({ "title": "title of doc 2", "content": "content of doc 2" }),
        ];

        writer
            .write(collection_id.clone(), docs.try_into().unwrap())
            .await
            .unwrap();

        let document_inserted = rec.try_recv().unwrap();
        assert!(matches!(
            document_inserted,
            WriteOperation::Collection(c_id, CollectionWriteOperation::InsertDocument { .. }) if c_id == collection_id
        ));

        let mut fields = vec![rec.try_recv().unwrap(), rec.try_recv().unwrap()];
        fields.sort_by_key(|o| match o {
            WriteOperation::Collection(
                _,
                CollectionWriteOperation::CreateField { field_name, .. },
            ) => field_name.clone(),
            _ => unreachable!(),
        });

        let content_field_id = check_create_field(&fields[0], &collection_id, "content");
        let title_field_id = check_create_field(&fields[1], &collection_id, "title");

        assert_ne!(content_field_id, title_field_id);
        assert_ne!(embedding_field_id, content_field_id);
        assert_ne!(embedding_field_id, title_field_id);

        let mut fields = vec![];
        while let std::result::Result::Ok(v) = rec.try_recv() {
            fields.push(v);
        }

        fields.sort_by(|a, b| {
            match (a, b) {
                (
                    WriteOperation::Collection(
                        _,
                        CollectionWriteOperation::IndexString {
                            doc_id: doc_id1,
                            field_id: a,
                            ..
                        },
                    ),
                    WriteOperation::Collection(
                        _,
                        CollectionWriteOperation::IndexString {
                            doc_id: doc_id2,
                            field_id: b,
                            ..
                        },
                    ),
                ) => {
                    if doc_id1.0 != doc_id2.0 {
                        return doc_id1.0.cmp(&doc_id2.0);
                    }

                    // title comes first
                    if *a == title_field_id && *b == content_field_id {
                        std::cmp::Ordering::Equal
                    } else if *a == title_field_id {
                        std::cmp::Ordering::Greater
                    } else if *b == title_field_id {
                        std::cmp::Ordering::Less
                    } else {
                        std::cmp::Ordering::Equal
                    }
                }
                (
                    WriteOperation::Collection(
                        _,
                        CollectionWriteOperation::IndexString {
                            doc_id: doc_id1, ..
                        },
                    ),
                    WriteOperation::Collection(
                        _,
                        CollectionWriteOperation::IndexEmbedding {
                            doc_id: doc_id2, ..
                        },
                    ),
                ) => {
                    if doc_id1.0 != doc_id2.0 {
                        return doc_id1.0.cmp(&doc_id2.0);
                    }
                    std::cmp::Ordering::Less
                }
                (
                    WriteOperation::Collection(
                        _,
                        CollectionWriteOperation::IndexEmbedding {
                            doc_id: doc_id1, ..
                        },
                    ),
                    WriteOperation::Collection(
                        _,
                        CollectionWriteOperation::IndexString {
                            doc_id: doc_id2, ..
                        },
                    ),
                ) => {
                    if doc_id1.0 != doc_id2.0 {
                        return doc_id1.0.cmp(&doc_id2.0);
                    }
                    std::cmp::Ordering::Greater
                }
                (
                    WriteOperation::Collection(
                        _,
                        CollectionWriteOperation::IndexEmbedding {
                            doc_id: doc_id1,
                            field_id: a,
                            ..
                        },
                    ),
                    WriteOperation::Collection(
                        _,
                        CollectionWriteOperation::IndexEmbedding {
                            doc_id: doc_id2,
                            field_id: b,
                            ..
                        },
                    ),
                ) => {
                    if doc_id1.0 != doc_id2.0 {
                        return doc_id1.0.cmp(&doc_id2.0);
                    }
                    if *a == embedding_field_id && *b == content_field_id {
                        std::cmp::Ordering::Equal
                    } else if *a == embedding_field_id {
                        std::cmp::Ordering::Less
                    } else if *b == title_field_id {
                        std::cmp::Ordering::Greater
                    } else {
                        std::cmp::Ordering::Equal
                    }
                }
                (
                    WriteOperation::Collection(_, CollectionWriteOperation::InsertDocument { .. }),
                    _,
                ) => std::cmp::Ordering::Greater,
                (
                    _,
                    WriteOperation::Collection(_, CollectionWriteOperation::InsertDocument { .. }),
                ) => std::cmp::Ordering::Less,
                a => unreachable!("{:?}", a),
            }
        });

        // Doc 1
        check_index_string(
            &fields[0],
            &collection_id,
            &DocumentId(0),
            &content_field_id,
            "content",
        );
        check_index_string(
            &fields[1],
            &collection_id,
            &DocumentId(0),
            &content_field_id,
            "doc",
        );
        check_index_string(
            &fields[2],
            &collection_id,
            &DocumentId(0),
            &content_field_id,
            "1",
        );

        check_index_string(
            &fields[3],
            &collection_id,
            &DocumentId(0),
            &title_field_id,
            "titl",
        );
        check_index_string(
            &fields[4],
            &collection_id,
            &DocumentId(0),
            &title_field_id,
            "title",
        );
        check_index_string(
            &fields[5],
            &collection_id,
            &DocumentId(0),
            &title_field_id,
            "doc",
        );
        check_index_string(
            &fields[6],
            &collection_id,
            &DocumentId(0),
            &title_field_id,
            "1",
        );

        check_index_embedding(
            &fields[7],
            &collection_id,
            &DocumentId(0),
            &embedding_field_id,
            384,
        );

        // Doc 2
        check_index_string(
            &fields[8],
            &collection_id,
            &DocumentId(1),
            &content_field_id,
            "content",
        );
        check_index_string(
            &fields[9],
            &collection_id,
            &DocumentId(1),
            &content_field_id,
            "doc",
        );
        check_index_string(
            &fields[10],
            &collection_id,
            &DocumentId(1),
            &content_field_id,
            "2",
        );

        check_index_string(
            &fields[11],
            &collection_id,
            &DocumentId(1),
            &title_field_id,
            "titl",
        );
        check_index_string(
            &fields[12],
            &collection_id,
            &DocumentId(1),
            &title_field_id,
            "title",
        );
        check_index_string(
            &fields[13],
            &collection_id,
            &DocumentId(1),
            &title_field_id,
            "doc",
        );
        check_index_string(
            &fields[14],
            &collection_id,
            &DocumentId(1),
            &title_field_id,
            "2",
        );

        check_index_embedding(
            &fields[15],
            &collection_id,
            &DocumentId(1),
            &embedding_field_id,
            384,
        );

        check_insert_doc(&fields[16], &DocumentId(1));

        assert_eq!(fields.len(), 17);
    }

    fn check_insert_doc(op: &WriteOperation, doc_id: &DocumentId) {
        match op {
            WriteOperation::Collection(
                _,
                CollectionWriteOperation::InsertDocument { doc_id: d, .. },
            ) => {
                assert_eq!(d, doc_id);
            }
            op => panic!("Not a InsertDocument operation: {:?}", op),
        }
    }

    fn check_index_embedding(
        op: &WriteOperation,
        collection_id: &CollectionId,
        doc_id: &DocumentId,
        field_id: &FieldId,
        embedding_len: usize,
    ) {
        match op {
            WriteOperation::Collection(
                c,
                CollectionWriteOperation::IndexEmbedding {
                    doc_id: d,
                    field_id: f,
                    value,
                },
            ) => {
                assert_eq!(c, collection_id);
                assert_eq!(d, doc_id);
                assert_eq!(f, field_id);
                assert_eq!(value.len(), embedding_len);
            }
            op => panic!("Not a IndexEmbedding operation: {:?}", op),
        }
    }

    fn check_index_string(
        op: &WriteOperation,
        collection_id: &CollectionId,
        doc_id: &DocumentId,
        field_id: &FieldId,
        value: &str,
    ) {
        match op {
            WriteOperation::Collection(
                c,
                CollectionWriteOperation::IndexString {
                    doc_id: d,
                    field_id: f,
                    ..
                },
            ) => {
                assert_eq!(c, collection_id);
                assert_eq!(d, doc_id);
                assert_eq!(f, field_id);
            }
            op => panic!("Not a IndexString operation: {:?}", op),
        }
    }

    fn check_create_field(
        op: &WriteOperation,
        collection_id: &CollectionId,
        field_name: &str,
    ) -> FieldId {
        match op {
            WriteOperation::Collection(
                c,
                CollectionWriteOperation::CreateField {
                    field_name: f,
                    field_id,
                    ..
                },
            ) => {
                assert_eq!(c, collection_id);
                assert_eq!(f, field_name);

                *field_id
            }
            op => panic!("Not a CreateField operation: {:?}", op),
        }
    }
    */
}
