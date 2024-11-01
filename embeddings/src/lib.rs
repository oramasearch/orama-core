use anyhow::{anyhow, Result};
use fastembed::{EmbeddingModel, InitOptions, TextEmbedding};
use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fmt;
use std::sync::Arc;
use strum::EnumIter;
use strum::IntoEnumIterator;

#[derive(Deserialize, Debug)]
pub struct EmbeddingsParams {
    model: OramaModels,
    intent: EncodingIntent,
    input: Vec<String>,
}

#[derive(Deserialize, Debug)]
#[serde(rename_all = "lowercase")]
pub enum EncodingIntent {
    Query,
    Passage,
}

#[derive(Serialize)]
pub struct EmbeddingsResponse {
    dimensions: i32,
    embeddings: Vec<Vec<f32>>,
}

#[derive(Deserialize, Debug, Hash, PartialEq, Eq, Copy, Clone, EnumIter)]
pub enum OramaModels {
    #[serde(rename = "gte-small")]
    GTESmall,
    #[serde(rename = "gte-base")]
    GTEBase,
    #[serde(rename = "gte-large")]
    GTELarge,
    #[serde(rename = "multilingual-e5-small")]
    MultilingualE5Small,
    #[serde(rename = "multilingual-e5-base")]
    MultilingualE5Base,
    #[serde(rename = "multilingual-e5-large")]
    MultilingualE5Large,
}

pub struct LoadedModels(HashMap<OramaModels, TextEmbedding>);

impl LoadedModels {
    fn embed(&self, model: OramaModels, input: Vec<String>) -> Result<Vec<Vec<f32>>> {
        let text_embedding = match self.0.get(&model) {
            Some(model) => model,
            None => return Err(anyhow!("Unable to retrieve embedding model: {model:?}")),
        };

        text_embedding.embed(input, None)
    }
}

impl Into<EmbeddingModel> for OramaModels {
    fn into(self) -> EmbeddingModel {
        match self {
            OramaModels::GTESmall => EmbeddingModel::BGESmallENV15,
            OramaModels::GTEBase => EmbeddingModel::BGEBaseENV15,
            OramaModels::GTELarge => EmbeddingModel::BGELargeENV15,
            OramaModels::MultilingualE5Small => EmbeddingModel::MultilingualE5Small,
            OramaModels::MultilingualE5Base => EmbeddingModel::MultilingualE5Base,
            OramaModels::MultilingualE5Large => EmbeddingModel::MultilingualE5Large,
        }
    }
}

impl OramaModels {
    fn normalize_input(self, intent: EncodingIntent, input: Vec<String>) -> Vec<String> {
        match self {
            OramaModels::MultilingualE5Small
            | OramaModels::MultilingualE5Base
            | OramaModels::MultilingualE5Large => input
                .into_iter()
                .map(|text| format!("{intent}: {text}"))
                .collect(),
            _ => input,
        }
    }

    fn max_input_tokens(self) -> usize {
        match self {
            OramaModels::GTESmall => 512,
            OramaModels::GTEBase => 512,
            OramaModels::GTELarge => 512,
            OramaModels::MultilingualE5Small => 512,
            OramaModels::MultilingualE5Base => 512,
            OramaModels::MultilingualE5Large => 512,
        }
    }

    fn dimensions(self) -> usize {
        match self {
            OramaModels::GTESmall => 384,
            OramaModels::GTEBase => 768,
            OramaModels::GTELarge => 1024,
            OramaModels::MultilingualE5Small => 384,
            OramaModels::MultilingualE5Base => 768,
            OramaModels::MultilingualE5Large => 1024,
        }
    }
}

impl fmt::Display for EncodingIntent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            EncodingIntent::Query => write!(f, "query"),
            EncodingIntent::Passage => write!(f, "passage"),
        }
    }
}

pub fn load_models() -> LoadedModels {
    let models: Vec<_> = OramaModels::iter().collect();

    let model_map: HashMap<OramaModels, TextEmbedding> = models
        .into_par_iter()
        .map(|model| {
            let initialized_model = TextEmbedding::try_new(
                InitOptions::new(model.into()).with_show_download_progress(true),
            )
            .unwrap();

            return (model, initialized_model);
        })
        .collect();

    LoadedModels(model_map)
}

pub async fn generate_embeddings(
    model: Arc<LoadedModels>,
    orama_model: OramaModels,
    input: Vec<String>,
    intent: EncodingIntent,
) -> Result<Vec<Vec<f32>>> {
    let normalized_input = orama_model.normalize_input(intent, input);

    tokio::task::spawn_blocking(move || model.embed(orama_model, normalized_input)).await?
}
