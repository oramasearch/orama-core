use std::sync::Arc;

use anyhow::{Context, Result};
use collection_manager::sides::{
    read::{CollectionsReader, IndexesConfig},
    write::{CollectionsWriter, WriteOperation},
    CollectionsWriterConfig,
};
use embeddings::{EmbeddingConfig, EmbeddingService};
use metrics_exporter_prometheus::PrometheusBuilder;
use serde::Deserialize;
use tokio::sync::broadcast::Receiver;
use tracing::info;
use web_server::{HttpConfig, WebServer};

pub mod indexes;
pub mod types;

pub mod code_parser;
pub mod nlp;

pub mod collection_manager;

pub mod web_server;

pub mod embeddings;

mod capped_heap;
pub mod js;

mod metrics;

mod file_utils;
pub mod ai_client;

#[cfg(any(test, feature = "benchmarking"))]
pub mod test_utils;

#[derive(Debug, Deserialize, Clone, PartialEq)]
pub enum SideChannelType {
    #[serde(rename = "in-memory")]
    InMemory,
}

#[derive(Debug, Deserialize, Clone)]
pub struct WriteSideConfig {
    pub output: SideChannelType,
    pub config: CollectionsWriterConfig,
}
#[derive(Debug, Deserialize, Clone)]
pub struct ReadSideConfig {
    pub input: SideChannelType,
    pub config: IndexesConfig,
}

#[derive(Debug, Deserialize, Clone)]
pub struct RustoramaConfig {
    pub http: HttpConfig,
    pub embeddings: EmbeddingConfig,
    pub writer_side: WriteSideConfig,
    pub reader_side: ReadSideConfig,
}

pub async fn start(config: RustoramaConfig) -> Result<()> {
    let prometheus_hadler = if config.http.with_prometheus {
        Some(
            PrometheusBuilder::new()
                .install_recorder()
                .context("failed to install recorder")?,
        )
    } else {
        None
    };

    let (writer, reader, mut receiver) = build_orama(config.clone()).await?;

    let collections_reader = reader.clone().unwrap();
    tokio::spawn(async move {
        while let Ok(op) = receiver.recv().await {
            collections_reader.update(op).await.expect("OUCH!");
        }
    });

    info!(
        "Starting web server on {}:{}",
        config.http.host, config.http.port
    );

    let web_server = WebServer::new(writer, reader, prometheus_hadler);
    web_server.start(config.http).await?;

    Ok(())
}

pub async fn build_orama(
    config: RustoramaConfig,
) -> Result<(
    Option<Arc<CollectionsWriter>>,
    Option<Arc<CollectionsReader>>,
    Receiver<WriteOperation>,
)> {
    let RustoramaConfig {
        embeddings: embedding_config,
        writer_side,
        reader_side,
        ..
    } = config;

    let embedding_service = EmbeddingService::try_new(embedding_config)
        .await
        .with_context(|| "Failed to initialize the EmbeddingService")?;
    let embedding_service = Arc::new(embedding_service);

    let (sender, receiver) = tokio::sync::broadcast::channel(10_000);

    assert_eq!(
        writer_side.output,
        SideChannelType::InMemory,
        "Only in-memory is supported"
    );
    assert_eq!(
        reader_side.input,
        SideChannelType::InMemory,
        "Only in-memory is supported"
    );

    let mut collections_writer =
        CollectionsWriter::new(sender, embedding_service.clone(), writer_side.config);
    collections_writer
        .load()
        .await
        .context("Cannot load collections writer")?;

    let mut collections_reader = CollectionsReader::try_new(embedding_service, reader_side.config)
        .context("Cannot create collections reader")?;

    collections_reader
        .load()
        .await
        .context("Cannot load collection reader")?;

    let collections_writer = Some(Arc::new(collections_writer));
    let collections_reader = Some(Arc::new(collections_reader));

    Ok((collections_writer, collections_reader, receiver))
}
