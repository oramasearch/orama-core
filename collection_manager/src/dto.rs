use std::collections::HashMap;

use nlp::Language;
use serde::{Deserialize, Serialize};
use types::FieldId;

#[derive(Debug, Serialize, Deserialize)]
pub enum LanguageDTO {
    English,
}

impl From<LanguageDTO> for Language {
    fn from(language: LanguageDTO) -> Self {
        match language {
            LanguageDTO::English => Language::English,
        }
    }
}
impl From<Language> for LanguageDTO {
    fn from(language: Language) -> Self {
        match language {
            Language::English => LanguageDTO::English,
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct CreateCollectionOptionDTO {
    pub id: String,
    pub description: Option<String>,
    pub language: Option<LanguageDTO>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct CollectionDTO {
    pub id: String,
    pub description: Option<String>,
    pub language: LanguageDTO,
    pub document_count: usize,
    pub string_fields: HashMap<String, FieldId>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct SearchParams {
    pub term: String,
}