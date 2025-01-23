# -*- coding: utf-8 -*-
# Generated by the protocol buffer compiler.  DO NOT EDIT!
# NO CHECKED-IN PROTOBUF GENCODE
# source: service.proto
# Protobuf Python Version: 5.29.0
"""Generated protocol buffer code."""
from google.protobuf import descriptor as _descriptor
from google.protobuf import descriptor_pool as _descriptor_pool
from google.protobuf import runtime_version as _runtime_version
from google.protobuf import symbol_database as _symbol_database
from google.protobuf.internal import builder as _builder

_runtime_version.ValidateProtobufRuntimeVersion(_runtime_version.Domain.PUBLIC, 5, 29, 0, "", "service.proto")
# @@protoc_insertion_point(imports)

_sym_db = _symbol_database.Default()


DESCRIPTOR = _descriptor_pool.Default().AddSerializedFile(
    b'\n\rservice.proto\x12\x10orama_ai_service"L\n\x13\x43onversationMessage\x12$\n\x04role\x18\x01 \x01(\x0e\x32\x16.orama_ai_service.Role\x12\x0f\n\x07\x63ontent\x18\x02 \x01(\t"G\n\x0c\x43onversation\x12\x37\n\x08messages\x18\x01 \x03(\x0b\x32%.orama_ai_service.ConversationMessage"}\n\x10\x45mbeddingRequest\x12+\n\x05model\x18\x01 \x01(\x0e\x32\x1c.orama_ai_service.OramaModel\x12\r\n\x05input\x18\x02 \x03(\t\x12-\n\x06intent\x18\x03 \x01(\x0e\x32\x1d.orama_ai_service.OramaIntent"_\n\x11\x45mbeddingResponse\x12\x36\n\x11\x65mbeddings_result\x18\x01 \x03(\x0b\x32\x1b.orama_ai_service.Embedding\x12\x12\n\ndimensions\x18\x02 \x01(\x05"\x1f\n\tEmbedding\x12\x12\n\nembeddings\x18\x01 \x03(\x02"%\n\x14PlannedAnswerRequest\x12\r\n\x05input\x18\x01 \x01(\t"%\n\x15PlannedAnswerResponse\x12\x0c\n\x04plan\x18\x01 \x01(\t"\x9f\x01\n\x0b\x43hatRequest\x12(\n\x05model\x18\x01 \x01(\x0e\x32\x19.orama_ai_service.LLMType\x12\x0e\n\x06prompt\x18\x02 \x01(\t\x12\x34\n\x0c\x63onversation\x18\x03 \x01(\x0b\x32\x1e.orama_ai_service.Conversation\x12\x14\n\x07\x63ontext\x18\x04 \x01(\tH\x00\x88\x01\x01\x42\n\n\x08_context"\x1c\n\x0c\x43hatResponse\x12\x0c\n\x04text\x18\x01 \x01(\t":\n\x12\x43hatStreamResponse\x12\x12\n\ntext_chunk\x18\x01 \x01(\t\x12\x10\n\x08is_final\x18\x02 \x01(\x08"%\n\x12HealthCheckRequest\x12\x0f\n\x07service\x18\x01 \x01(\t"%\n\x13HealthCheckResponse\x12\x0e\n\x06status\x18\x01 \x01(\t*\x7f\n\nOramaModel\x12\x0c\n\x08\x42GESmall\x10\x00\x12\x0b\n\x07\x42GEBase\x10\x01\x12\x0c\n\x08\x42GELarge\x10\x02\x12\x17\n\x13MultilingualE5Small\x10\x03\x12\x16\n\x12MultilingualE5Base\x10\x04\x12\x17\n\x13MultilingualE5Large\x10\x05*%\n\x0bOramaIntent\x12\t\n\x05query\x10\x00\x12\x0b\n\x07passage\x10\x01*U\n\x07LLMType\x12\x15\n\x11\x63ontent_expansion\x10\x00\x12\x1b\n\x17google_query_translator\x10\x01\x12\n\n\x06vision\x10\x02\x12\n\n\x06\x61nswer\x10\x03*+\n\x04Role\x12\x08\n\x04USER\x10\x00\x12\r\n\tASSISTANT\x10\x01\x12\n\n\x06SYSTEM\x10\x02\x32\xbf\x03\n\nLLMService\x12Z\n\x0b\x43heckHealth\x12$.orama_ai_service.HealthCheckRequest\x1a%.orama_ai_service.HealthCheckResponse\x12W\n\x0cGetEmbedding\x12".orama_ai_service.EmbeddingRequest\x1a#.orama_ai_service.EmbeddingResponse\x12\x45\n\x04\x43hat\x12\x1d.orama_ai_service.ChatRequest\x1a\x1e.orama_ai_service.ChatResponse\x12S\n\nChatStream\x12\x1d.orama_ai_service.ChatRequest\x1a$.orama_ai_service.ChatStreamResponse0\x01\x12`\n\rPlannedAnswer\x12&.orama_ai_service.PlannedAnswerRequest\x1a\'.orama_ai_service.PlannedAnswerResponseb\x06proto3'
)

_globals = globals()
_builder.BuildMessageAndEnumDescriptors(DESCRIPTOR, _globals)
_builder.BuildTopDescriptorsAndMessages(DESCRIPTOR, "service_pb2", _globals)
if not _descriptor._USE_C_DESCRIPTORS:
    DESCRIPTOR._loaded_options = None
    _globals["_ORAMAMODEL"]._serialized_start = 851
    _globals["_ORAMAMODEL"]._serialized_end = 978
    _globals["_ORAMAINTENT"]._serialized_start = 980
    _globals["_ORAMAINTENT"]._serialized_end = 1017
    _globals["_LLMTYPE"]._serialized_start = 1019
    _globals["_LLMTYPE"]._serialized_end = 1104
    _globals["_ROLE"]._serialized_start = 1106
    _globals["_ROLE"]._serialized_end = 1149
    _globals["_CONVERSATIONMESSAGE"]._serialized_start = 35
    _globals["_CONVERSATIONMESSAGE"]._serialized_end = 111
    _globals["_CONVERSATION"]._serialized_start = 113
    _globals["_CONVERSATION"]._serialized_end = 184
    _globals["_EMBEDDINGREQUEST"]._serialized_start = 186
    _globals["_EMBEDDINGREQUEST"]._serialized_end = 311
    _globals["_EMBEDDINGRESPONSE"]._serialized_start = 313
    _globals["_EMBEDDINGRESPONSE"]._serialized_end = 408
    _globals["_EMBEDDING"]._serialized_start = 410
    _globals["_EMBEDDING"]._serialized_end = 441
    _globals["_PLANNEDANSWERREQUEST"]._serialized_start = 443
    _globals["_PLANNEDANSWERREQUEST"]._serialized_end = 480
    _globals["_PLANNEDANSWERRESPONSE"]._serialized_start = 482
    _globals["_PLANNEDANSWERRESPONSE"]._serialized_end = 519
    _globals["_CHATREQUEST"]._serialized_start = 522
    _globals["_CHATREQUEST"]._serialized_end = 681
    _globals["_CHATRESPONSE"]._serialized_start = 683
    _globals["_CHATRESPONSE"]._serialized_end = 711
    _globals["_CHATSTREAMRESPONSE"]._serialized_start = 713
    _globals["_CHATSTREAMRESPONSE"]._serialized_end = 771
    _globals["_HEALTHCHECKREQUEST"]._serialized_start = 773
    _globals["_HEALTHCHECKREQUEST"]._serialized_end = 810
    _globals["_HEALTHCHECKRESPONSE"]._serialized_start = 812
    _globals["_HEALTHCHECKRESPONSE"]._serialized_end = 849
    _globals["_LLMSERVICE"]._serialized_start = 1152
    _globals["_LLMSERVICE"]._serialized_end = 1599
# @@protoc_insertion_point(module_scope)
