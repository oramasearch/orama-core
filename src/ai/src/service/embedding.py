import uvicorn
import threading
from concurrent.futures import ThreadPoolExecutor

from src.embeddings.models import (
    EmbeddingsModels,
    extend_fastembed_supported_models,
    ModelGroups,
)
from src.embeddings.embeddings import initialize_thread_executor
from src.api.app import create_app
from src.grpc.server import serve


class EmbeddingService:
    def __init__(self, config):
        self.config = config
        self.thread_executor = ThreadPoolExecutor(max_workers=config.total_threads // 2)
        self.embeddings_service = self._initialize_embeddings_service()
        self.app = create_app(self)

    def _initialize_embeddings_service(self):
        import os

        os.environ["ONNXRUNTIME_PROVIDERS"] = "CPUExecutionProvider"

        extend_fastembed_supported_models()
        initialize_thread_executor(max_workers=self.config.total_threads // 2)
        return EmbeddingsModels(self.config, selected_models=ModelGroups[self.config.default_model_group].value)

    def start(self):
        grpc_thread = threading.Thread(target=lambda: serve(self.config, self.embeddings_service), daemon=True)
        grpc_thread.start()

        uvicorn.run(self.app, host=self.config.host, port=int(self.config.http_port), log_level="error")
