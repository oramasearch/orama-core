# Generated by the gRPC Python protocol compiler plugin. DO NOT EDIT!
"""Client and server classes corresponding to protobuf-defined services."""
import grpc
import warnings

import service_pb2 as service__pb2

GRPC_GENERATED_VERSION = "1.69.0"
GRPC_VERSION = grpc.__version__
_version_not_supported = False

try:
    from grpc._utilities import first_version_is_lower

    _version_not_supported = first_version_is_lower(GRPC_VERSION, GRPC_GENERATED_VERSION)
except ImportError:
    _version_not_supported = True

if _version_not_supported:
    raise RuntimeError(
        f"The grpc package installed is at version {GRPC_VERSION},"
        + f" but the generated code in service_pb2_grpc.py depends on"
        + f" grpcio>={GRPC_GENERATED_VERSION}."
        + f" Please upgrade your grpc module to grpcio>={GRPC_GENERATED_VERSION}"
        + f" or downgrade your generated code using grpcio-tools<={GRPC_VERSION}."
    )


class HealthCheckServiceStub(object):
    """Missing associated documentation comment in .proto file."""

    def __init__(self, channel):
        """Constructor.

        Args:
            channel: A grpc.Channel.
        """
        self.CheckHealth = channel.unary_unary(
            "/orama_ai_service.HealthCheckService/CheckHealth",
            request_serializer=service__pb2.HealthCheckRequest.SerializeToString,
            response_deserializer=service__pb2.HealthCheckResponse.FromString,
            _registered_method=True,
        )


class HealthCheckServiceServicer(object):
    """Missing associated documentation comment in .proto file."""

    def CheckHealth(self, request, context):
        """Missing associated documentation comment in .proto file."""
        context.set_code(grpc.StatusCode.UNIMPLEMENTED)
        context.set_details("Method not implemented!")
        raise NotImplementedError("Method not implemented!")


def add_HealthCheckServiceServicer_to_server(servicer, server):
    rpc_method_handlers = {
        "CheckHealth": grpc.unary_unary_rpc_method_handler(
            servicer.CheckHealth,
            request_deserializer=service__pb2.HealthCheckRequest.FromString,
            response_serializer=service__pb2.HealthCheckResponse.SerializeToString,
        ),
    }
    generic_handler = grpc.method_handlers_generic_handler("orama_ai_service.HealthCheckService", rpc_method_handlers)
    server.add_generic_rpc_handlers((generic_handler,))
    server.add_registered_method_handlers("orama_ai_service.HealthCheckService", rpc_method_handlers)


# This class is part of an EXPERIMENTAL API.
class HealthCheckService(object):
    """Missing associated documentation comment in .proto file."""

    @staticmethod
    def CheckHealth(
        request,
        target,
        options=(),
        channel_credentials=None,
        call_credentials=None,
        insecure=False,
        compression=None,
        wait_for_ready=None,
        timeout=None,
        metadata=None,
    ):
        return grpc.experimental.unary_unary(
            request,
            target,
            "/orama_ai_service.HealthCheckService/CheckHealth",
            service__pb2.HealthCheckRequest.SerializeToString,
            service__pb2.HealthCheckResponse.FromString,
            options,
            channel_credentials,
            insecure,
            call_credentials,
            compression,
            wait_for_ready,
            timeout,
            metadata,
            _registered_method=True,
        )


class CalculateEmbeddingsServiceStub(object):
    """Missing associated documentation comment in .proto file."""

    def __init__(self, channel):
        """Constructor.

        Args:
            channel: A grpc.Channel.
        """
        self.GetEmbedding = channel.unary_unary(
            "/orama_ai_service.CalculateEmbeddingsService/GetEmbedding",
            request_serializer=service__pb2.EmbeddingRequest.SerializeToString,
            response_deserializer=service__pb2.EmbeddingResponse.FromString,
            _registered_method=True,
        )


class CalculateEmbeddingsServiceServicer(object):
    """Missing associated documentation comment in .proto file."""

    def GetEmbedding(self, request, context):
        """Missing associated documentation comment in .proto file."""
        context.set_code(grpc.StatusCode.UNIMPLEMENTED)
        context.set_details("Method not implemented!")
        raise NotImplementedError("Method not implemented!")


def add_CalculateEmbeddingsServiceServicer_to_server(servicer, server):
    rpc_method_handlers = {
        "GetEmbedding": grpc.unary_unary_rpc_method_handler(
            servicer.GetEmbedding,
            request_deserializer=service__pb2.EmbeddingRequest.FromString,
            response_serializer=service__pb2.EmbeddingResponse.SerializeToString,
        ),
    }
    generic_handler = grpc.method_handlers_generic_handler(
        "orama_ai_service.CalculateEmbeddingsService", rpc_method_handlers
    )
    server.add_generic_rpc_handlers((generic_handler,))
    server.add_registered_method_handlers("orama_ai_service.CalculateEmbeddingsService", rpc_method_handlers)


# This class is part of an EXPERIMENTAL API.
class CalculateEmbeddingsService(object):
    """Missing associated documentation comment in .proto file."""

    @staticmethod
    def GetEmbedding(
        request,
        target,
        options=(),
        channel_credentials=None,
        call_credentials=None,
        insecure=False,
        compression=None,
        wait_for_ready=None,
        timeout=None,
        metadata=None,
    ):
        return grpc.experimental.unary_unary(
            request,
            target,
            "/orama_ai_service.CalculateEmbeddingsService/GetEmbedding",
            service__pb2.EmbeddingRequest.SerializeToString,
            service__pb2.EmbeddingResponse.FromString,
            options,
            channel_credentials,
            insecure,
            call_credentials,
            compression,
            wait_for_ready,
            timeout,
            metadata,
            _registered_method=True,
        )


class LLMServiceStub(object):
    """Missing associated documentation comment in .proto file."""

    def __init__(self, channel):
        """Constructor.

        Args:
            channel: A grpc.Channel.
        """
        self.CallLLM = channel.unary_unary(
            "/orama_ai_service.LLMService/CallLLM",
            request_serializer=service__pb2.LLMRequest.SerializeToString,
            response_deserializer=service__pb2.LLMResponse.FromString,
            _registered_method=True,
        )
        self.CallLLMStream = channel.unary_stream(
            "/orama_ai_service.LLMService/CallLLMStream",
            request_serializer=service__pb2.LLMRequest.SerializeToString,
            response_deserializer=service__pb2.LLMStreamResponse.FromString,
            _registered_method=True,
        )


class LLMServiceServicer(object):
    """Missing associated documentation comment in .proto file."""

    def CallLLM(self, request, context):
        """Missing associated documentation comment in .proto file."""
        context.set_code(grpc.StatusCode.UNIMPLEMENTED)
        context.set_details("Method not implemented!")
        raise NotImplementedError("Method not implemented!")

    def CallLLMStream(self, request, context):
        """Missing associated documentation comment in .proto file."""
        context.set_code(grpc.StatusCode.UNIMPLEMENTED)
        context.set_details("Method not implemented!")
        raise NotImplementedError("Method not implemented!")


def add_LLMServiceServicer_to_server(servicer, server):
    rpc_method_handlers = {
        "CallLLM": grpc.unary_unary_rpc_method_handler(
            servicer.CallLLM,
            request_deserializer=service__pb2.LLMRequest.FromString,
            response_serializer=service__pb2.LLMResponse.SerializeToString,
        ),
        "CallLLMStream": grpc.unary_stream_rpc_method_handler(
            servicer.CallLLMStream,
            request_deserializer=service__pb2.LLMRequest.FromString,
            response_serializer=service__pb2.LLMStreamResponse.SerializeToString,
        ),
    }
    generic_handler = grpc.method_handlers_generic_handler("orama_ai_service.LLMService", rpc_method_handlers)
    server.add_generic_rpc_handlers((generic_handler,))
    server.add_registered_method_handlers("orama_ai_service.LLMService", rpc_method_handlers)


# This class is part of an EXPERIMENTAL API.
class LLMService(object):
    """Missing associated documentation comment in .proto file."""

    @staticmethod
    def CallLLM(
        request,
        target,
        options=(),
        channel_credentials=None,
        call_credentials=None,
        insecure=False,
        compression=None,
        wait_for_ready=None,
        timeout=None,
        metadata=None,
    ):
        return grpc.experimental.unary_unary(
            request,
            target,
            "/orama_ai_service.LLMService/CallLLM",
            service__pb2.LLMRequest.SerializeToString,
            service__pb2.LLMResponse.FromString,
            options,
            channel_credentials,
            insecure,
            call_credentials,
            compression,
            wait_for_ready,
            timeout,
            metadata,
            _registered_method=True,
        )

    @staticmethod
    def CallLLMStream(
        request,
        target,
        options=(),
        channel_credentials=None,
        call_credentials=None,
        insecure=False,
        compression=None,
        wait_for_ready=None,
        timeout=None,
        metadata=None,
    ):
        return grpc.experimental.unary_stream(
            request,
            target,
            "/orama_ai_service.LLMService/CallLLMStream",
            service__pb2.LLMRequest.SerializeToString,
            service__pb2.LLMStreamResponse.FromString,
            options,
            channel_credentials,
            insecure,
            call_credentials,
            compression,
            wait_for_ready,
            timeout,
            metadata,
            _registered_method=True,
        )


class VisionServiceStub(object):
    """Missing associated documentation comment in .proto file."""

    def __init__(self, channel):
        """Constructor.

        Args:
            channel: A grpc.Channel.
        """
        self.CallVision = channel.unary_unary(
            "/orama_ai_service.VisionService/CallVision",
            request_serializer=service__pb2.VisionRequest.SerializeToString,
            response_deserializer=service__pb2.VisionResponse.FromString,
            _registered_method=True,
        )


class VisionServiceServicer(object):
    """Missing associated documentation comment in .proto file."""

    def CallVision(self, request, context):
        """Missing associated documentation comment in .proto file."""
        context.set_code(grpc.StatusCode.UNIMPLEMENTED)
        context.set_details("Method not implemented!")
        raise NotImplementedError("Method not implemented!")


def add_VisionServiceServicer_to_server(servicer, server):
    rpc_method_handlers = {
        "CallVision": grpc.unary_unary_rpc_method_handler(
            servicer.CallVision,
            request_deserializer=service__pb2.VisionRequest.FromString,
            response_serializer=service__pb2.VisionResponse.SerializeToString,
        ),
    }
    generic_handler = grpc.method_handlers_generic_handler("orama_ai_service.VisionService", rpc_method_handlers)
    server.add_generic_rpc_handlers((generic_handler,))
    server.add_registered_method_handlers("orama_ai_service.VisionService", rpc_method_handlers)


# This class is part of an EXPERIMENTAL API.
class VisionService(object):
    """Missing associated documentation comment in .proto file."""

    @staticmethod
    def CallVision(
        request,
        target,
        options=(),
        channel_credentials=None,
        call_credentials=None,
        insecure=False,
        compression=None,
        wait_for_ready=None,
        timeout=None,
        metadata=None,
    ):
        return grpc.experimental.unary_unary(
            request,
            target,
            "/orama_ai_service.VisionService/CallVision",
            service__pb2.VisionRequest.SerializeToString,
            service__pb2.VisionResponse.FromString,
            options,
            channel_credentials,
            insecure,
            call_credentials,
            compression,
            wait_for_ready,
            timeout,
            metadata,
            _registered_method=True,
        )
