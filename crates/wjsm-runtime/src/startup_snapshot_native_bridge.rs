//! Maps between runtime `NativeCallable` and `wjsm_snapshot_format::SnapshotNativeCallable`.

use anyhow::{Result, bail};

use crate::runtime_node_child_process::ChildProcessMethodKind;
use crate::runtime_node_dgram::DgramMethodKind;
use crate::runtime_node_fs::FsMethodKind;
use crate::runtime_node_net::NetMethodKind;
use crate::runtime_node_perf_hooks::PerfHooksMethodKind;
use crate::runtime_node_tls::TlsMethodKind;
use crate::runtime_node_vm::VmMethodKind;
use crate::runtime_node_worker_threads::WorkerThreadsMethodKind;
use crate::runtime_node_zlib::ZlibMethodKind;
use crate::types::{NativeCallable, OsInfoKind, TypedArrayConstructorKind};
use wjsm_snapshot_format::SnapshotNativeCallable;

pub(crate) trait SnapshotNativeCallableBridge {
    fn into_native_callable(self, method: u8) -> NativeCallable;
    fn try_from_native_callable(nc: &NativeCallable) -> Result<Self>
    where
        Self: Sized;
}

impl SnapshotNativeCallableBridge for SnapshotNativeCallable {
    fn into_native_callable(self, method: u8) -> NativeCallable {
        match self {
            Self::EvalIndirect => NativeCallable::EvalIndirect,
            Self::AsyncIteratorProtoSymbolAsyncIterator => {
                NativeCallable::AsyncIteratorProtoSymbolAsyncIterator
            }
            Self::IteratorProtoSymbolIterator => NativeCallable::IteratorProtoSymbolIterator,
            Self::ArrayProtoValues => NativeCallable::ArrayProtoValues,
            Self::ArrayProtoKeys => NativeCallable::ArrayProtoKeys,
            Self::ArrayProtoEntries => NativeCallable::ArrayProtoEntries,
            Self::ArrayConstructor => NativeCallable::ArrayConstructor,
            Self::ObjectConstructor => NativeCallable::ObjectConstructor,
            Self::ObjectProtoToString => NativeCallable::ObjectProtoToString,
            Self::ObjectProtoValueOf => NativeCallable::ObjectProtoValueOf,
            Self::FunctionProtoCall => NativeCallable::FunctionProtoCall,
            Self::FunctionProtoApply => NativeCallable::FunctionProtoApply,
            Self::FunctionProtoBind => NativeCallable::FunctionProtoBind,
            Self::FunctionConstructor => NativeCallable::FunctionConstructor,
            Self::StringConstructor => NativeCallable::StringConstructor,
            Self::BooleanConstructor => NativeCallable::BooleanConstructor,
            Self::NumberConstructor => NativeCallable::NumberConstructor,
            Self::SymbolConstructor => NativeCallable::SymbolConstructor,
            Self::BigIntConstructor => NativeCallable::BigIntConstructor,
            Self::RegExpConstructor => NativeCallable::RegExpConstructor,
            Self::ErrorConstructor => NativeCallable::ErrorConstructor,
            Self::TypeErrorConstructor => NativeCallable::TypeErrorConstructor,
            Self::RangeErrorConstructor => NativeCallable::RangeErrorConstructor,
            Self::SyntaxErrorConstructor => NativeCallable::SyntaxErrorConstructor,
            Self::ReferenceErrorConstructor => NativeCallable::ReferenceErrorConstructor,
            Self::URIErrorConstructor => NativeCallable::URIErrorConstructor,
            Self::EvalErrorConstructor => NativeCallable::EvalErrorConstructor,
            Self::AggregateErrorConstructor => NativeCallable::AggregateErrorConstructor,
            Self::MapConstructor => NativeCallable::MapConstructor,
            Self::SetConstructor => NativeCallable::SetConstructor,
            Self::WeakMapConstructor => NativeCallable::WeakMapConstructor,
            Self::WeakSetConstructor => NativeCallable::WeakSetConstructor,
            Self::WeakRefConstructor => NativeCallable::WeakRefConstructor,
            Self::FinalizationRegistryConstructor => {
                NativeCallable::FinalizationRegistryConstructor
            }
            Self::DateConstructorGlobal => NativeCallable::DateConstructorGlobal,
            Self::PromiseConstructor => NativeCallable::PromiseConstructor,
            Self::ArrayBufferConstructorGlobal => NativeCallable::ArrayBufferConstructorGlobal,
            Self::DataViewConstructorGlobal => NativeCallable::DataViewConstructorGlobal,
            Self::BigInt64ArrayConstructor => NativeCallable::BigInt64ArrayConstructor,
            Self::BigUint64ArrayConstructor => NativeCallable::BigUint64ArrayConstructor,
            Self::ProxyConstructor => NativeCallable::ProxyConstructor,
            Self::GcCollect => NativeCallable::GcCollect,
            Self::SharedArrayBufferConstructor => NativeCallable::SharedArrayBufferConstructor,
            Self::AtomicsGlobal => NativeCallable::AtomicsGlobal,
            Self::AgentStart => NativeCallable::AgentStart,
            Self::AgentBroadcast => NativeCallable::AgentBroadcast,
            Self::AgentReceiveBroadcast => NativeCallable::AgentReceiveBroadcast,
            Self::AgentGetReport => NativeCallable::AgentGetReport,
            Self::AgentReport => NativeCallable::AgentReport,
            Self::AgentSleep => NativeCallable::AgentSleep,
            Self::AgentMonotonicNow => NativeCallable::AgentMonotonicNow,
            Self::HeadersConstructor => NativeCallable::HeadersConstructor,
            Self::ResponseConstructor => NativeCallable::ResponseConstructor,
            Self::RequestConstructor => NativeCallable::RequestConstructor,
            Self::AbortControllerConstructor => NativeCallable::AbortControllerConstructor,
            Self::ReadableStreamConstructor => NativeCallable::ReadableStreamConstructor,
            Self::WritableStreamConstructor => NativeCallable::WritableStreamConstructor,
            Self::TransformStreamConstructor => NativeCallable::TransformStreamConstructor,
            Self::CountQueuingStrategyConstructor => {
                NativeCallable::CountQueuingStrategyConstructor
            }
            Self::ByteLengthQueuingStrategyConstructor => {
                NativeCallable::ByteLengthQueuingStrategyConstructor
            }
            Self::StubGlobal => NativeCallable::StubGlobal(()),
            Self::BigIntPrimitiveMethod => NativeCallable::BigIntPrimitiveMethod { method },
            Self::NumberPrimitiveMethod => NativeCallable::NumberPrimitiveMethod { method },
            Self::SymbolPrimitiveMethod => NativeCallable::SymbolPrimitiveMethod { method },
            Self::RegExpPrimitiveMethod => NativeCallable::RegExpPrimitiveMethod { method },
            Self::SymbolProtoDescriptionGetter => NativeCallable::SymbolProtoDescriptionGetter,
            Self::SymbolProtoToPrimitive => NativeCallable::SymbolProtoToPrimitive,
            Self::ArgumentsStrictCalleeGetter => NativeCallable::ArgumentsStrictCalleeGetter,
            Self::TypedArrayConstructor => NativeCallable::TypedArrayConstructor(
                TypedArrayConstructorKind::from_index(method as usize)
                    .unwrap_or(TypedArrayConstructorKind::Uint8),
            ),
            Self::ErrorProtoToString => NativeCallable::ErrorProtoToString,
            Self::BufferConstructor => NativeCallable::BufferConstructor,
            Self::TextEncoderConstructor => NativeCallable::TextEncoderConstructor,
            Self::TextDecoderConstructor => NativeCallable::TextDecoderConstructor,
            Self::StructuredClone => NativeCallable::StructuredClone,
            Self::Atob => NativeCallable::Atob,
            Self::Btoa => NativeCallable::Btoa,
            Self::QueueMicrotask => NativeCallable::QueueMicrotask,
            Self::PerformanceNow => NativeCallable::PerformanceNow,
            Self::PerfHooksMethod => NativeCallable::PerfHooksMethod {
                kind: PerfHooksMethodKind::from_method(method)
                    .unwrap_or(PerfHooksMethodKind::TimeOrigin),
            },
            Self::OsInfo => NativeCallable::OsInfo {
                kind: OsInfoKind::from_method(method).unwrap_or(OsInfoKind::Tmpdir),
            },
            Self::FsMethod => NativeCallable::FsMethod {
                kind: FsMethodKind::from_method(method).unwrap_or(FsMethodKind::ReadFileSync),
            },
            Self::ZlibMethod => NativeCallable::ZlibMethod {
                kind: ZlibMethodKind::from_method(method).unwrap_or(ZlibMethodKind::GzipSync),
            },
            Self::ChildProcessMethod => NativeCallable::ChildProcessMethod {
                kind: ChildProcessMethodKind::from_method(method)
                    .unwrap_or(ChildProcessMethodKind::SpawnSync),
            },
            Self::NetMethod => NativeCallable::NetMethod {
                kind: NetMethodKind::from_method(method).unwrap_or(NetMethodKind::Connect),
            },
            Self::VmMethod => NativeCallable::VmMethod {
                kind: VmMethodKind::from_method(method).unwrap_or(VmMethodKind::CreateContext),
            },
            Self::DgramMethod => NativeCallable::DgramMethod {
                kind: DgramMethodKind::from_method(method).unwrap_or(DgramMethodKind::Bind),
            },
            Self::TlsMethod => NativeCallable::TlsMethod {
                kind: TlsMethodKind::from_method(method).unwrap_or(TlsMethodKind::Connect),
            },
            Self::WorkerThreadsMethod => NativeCallable::WorkerThreadsMethod {
                kind: WorkerThreadsMethodKind::from_method(method)
                    .unwrap_or(WorkerThreadsMethodKind::CreateMessageChannel),
            },
        }
    }

    fn try_from_native_callable(nc: &NativeCallable) -> Result<Self> {
        let result = match nc {
            NativeCallable::EvalIndirect => Self::EvalIndirect,
            NativeCallable::AsyncIteratorProtoSymbolAsyncIterator => {
                Self::AsyncIteratorProtoSymbolAsyncIterator
            }
            NativeCallable::IteratorProtoSymbolIterator => Self::IteratorProtoSymbolIterator,
            NativeCallable::ArrayProtoValues => Self::ArrayProtoValues,
            NativeCallable::ArrayProtoKeys => Self::ArrayProtoKeys,
            NativeCallable::ArrayProtoEntries => Self::ArrayProtoEntries,
            NativeCallable::ArrayConstructor => Self::ArrayConstructor,
            NativeCallable::ObjectConstructor => Self::ObjectConstructor,
            NativeCallable::ObjectProtoToString => Self::ObjectProtoToString,
            NativeCallable::ErrorProtoToString => Self::ErrorProtoToString,
            NativeCallable::ObjectProtoValueOf => Self::ObjectProtoValueOf,
            NativeCallable::FunctionProtoCall => Self::FunctionProtoCall,
            NativeCallable::FunctionProtoApply => Self::FunctionProtoApply,
            NativeCallable::FunctionProtoBind => Self::FunctionProtoBind,
            NativeCallable::FunctionConstructor => Self::FunctionConstructor,
            NativeCallable::StringConstructor => Self::StringConstructor,
            NativeCallable::BooleanConstructor => Self::BooleanConstructor,
            NativeCallable::NumberConstructor => Self::NumberConstructor,
            NativeCallable::SymbolConstructor => Self::SymbolConstructor,
            NativeCallable::BigIntConstructor => Self::BigIntConstructor,
            NativeCallable::RegExpConstructor => Self::RegExpConstructor,
            NativeCallable::ErrorConstructor => Self::ErrorConstructor,
            NativeCallable::TypeErrorConstructor => Self::TypeErrorConstructor,
            NativeCallable::RangeErrorConstructor => Self::RangeErrorConstructor,
            NativeCallable::SyntaxErrorConstructor => Self::SyntaxErrorConstructor,
            NativeCallable::ReferenceErrorConstructor => Self::ReferenceErrorConstructor,
            NativeCallable::URIErrorConstructor => Self::URIErrorConstructor,
            NativeCallable::EvalErrorConstructor => Self::EvalErrorConstructor,
            NativeCallable::AggregateErrorConstructor => Self::AggregateErrorConstructor,
            NativeCallable::MapConstructor => Self::MapConstructor,
            NativeCallable::SetConstructor => Self::SetConstructor,
            NativeCallable::WeakMapConstructor => Self::WeakMapConstructor,
            NativeCallable::WeakSetConstructor => Self::WeakSetConstructor,
            NativeCallable::WeakRefConstructor => Self::WeakRefConstructor,
            NativeCallable::FinalizationRegistryConstructor => {
                Self::FinalizationRegistryConstructor
            }
            NativeCallable::DateConstructorGlobal => Self::DateConstructorGlobal,
            NativeCallable::PromiseConstructor => Self::PromiseConstructor,
            NativeCallable::ArrayBufferConstructorGlobal => Self::ArrayBufferConstructorGlobal,
            NativeCallable::DataViewConstructorGlobal => Self::DataViewConstructorGlobal,
            NativeCallable::BigInt64ArrayConstructor => Self::BigInt64ArrayConstructor,
            NativeCallable::BigUint64ArrayConstructor => Self::BigUint64ArrayConstructor,
            NativeCallable::ProxyConstructor => Self::ProxyConstructor,
            NativeCallable::GcCollect => Self::GcCollect,
            NativeCallable::SharedArrayBufferConstructor => Self::SharedArrayBufferConstructor,
            NativeCallable::AtomicsGlobal => Self::AtomicsGlobal,
            NativeCallable::AgentStart => Self::AgentStart,
            NativeCallable::AgentBroadcast => Self::AgentBroadcast,
            NativeCallable::AgentReceiveBroadcast => Self::AgentReceiveBroadcast,
            NativeCallable::AgentGetReport => Self::AgentGetReport,
            NativeCallable::AgentReport => Self::AgentReport,
            NativeCallable::AgentSleep => Self::AgentSleep,
            NativeCallable::AgentMonotonicNow => Self::AgentMonotonicNow,
            NativeCallable::HeadersConstructor => Self::HeadersConstructor,
            NativeCallable::ResponseConstructor => Self::ResponseConstructor,
            NativeCallable::RequestConstructor => Self::RequestConstructor,
            NativeCallable::AbortControllerConstructor => Self::AbortControllerConstructor,
            NativeCallable::ReadableStreamConstructor => Self::ReadableStreamConstructor,
            NativeCallable::WritableStreamConstructor => Self::WritableStreamConstructor,
            NativeCallable::TransformStreamConstructor => Self::TransformStreamConstructor,
            NativeCallable::CountQueuingStrategyConstructor => {
                Self::CountQueuingStrategyConstructor
            }
            NativeCallable::ByteLengthQueuingStrategyConstructor => {
                Self::ByteLengthQueuingStrategyConstructor
            }
            NativeCallable::StubGlobal(()) => Self::StubGlobal,
            NativeCallable::ArgumentsStrictCalleeGetter => Self::ArgumentsStrictCalleeGetter,
            NativeCallable::BigIntPrimitiveMethod { method: _ } => Self::BigIntPrimitiveMethod,
            NativeCallable::NumberPrimitiveMethod { method: _ } => Self::NumberPrimitiveMethod,
            NativeCallable::SymbolPrimitiveMethod { method: _ } => Self::SymbolPrimitiveMethod,
            NativeCallable::RegExpPrimitiveMethod { method: _ } => Self::RegExpPrimitiveMethod,
            NativeCallable::SymbolProtoDescriptionGetter => Self::SymbolProtoDescriptionGetter,
            NativeCallable::SymbolProtoToPrimitive => Self::SymbolProtoToPrimitive,
            NativeCallable::TypedArrayConstructor(_) => Self::TypedArrayConstructor,
            NativeCallable::BufferConstructor => Self::BufferConstructor,
            NativeCallable::TextEncoderConstructor => Self::TextEncoderConstructor,
            NativeCallable::TextDecoderConstructor => Self::TextDecoderConstructor,
            NativeCallable::StructuredClone => Self::StructuredClone,
            NativeCallable::Atob => Self::Atob,
            NativeCallable::Btoa => Self::Btoa,
            NativeCallable::QueueMicrotask => Self::QueueMicrotask,
            NativeCallable::PerformanceNow => Self::PerformanceNow,
            NativeCallable::PerfHooksMethod { kind: _ } => Self::PerfHooksMethod,
            NativeCallable::OsInfo { kind: _ } => Self::OsInfo,
            NativeCallable::FsMethod { kind: _ } => Self::FsMethod,
            NativeCallable::ZlibMethod { kind: _ } => Self::ZlibMethod,
            NativeCallable::ChildProcessMethod { kind: _ } => Self::ChildProcessMethod,
            NativeCallable::NetMethod { kind: _ } => Self::NetMethod,
            NativeCallable::VmMethod { kind: _ } => Self::VmMethod,
            // AsyncHooksMethod 含动态 ALS 状态，禁止入 snapshot
            NativeCallable::DgramMethod { kind: _ } => Self::DgramMethod,
            NativeCallable::TlsMethod { kind: _ } => Self::TlsMethod,
            NativeCallable::WorkerThreadsMethod { kind: _ } => Self::WorkerThreadsMethod,
            NativeCallable::CjsRequire { .. }
            | NativeCallable::CjsRequireResolve { .. }
            | NativeCallable::CjsRequireResolvePaths { .. }
            | NativeCallable::ImportMetaResolve { .. }
            | NativeCallable::CjsRequireCacheTrap { .. }
            | NativeCallable::AsyncHooksMethod { .. }
            | NativeCallable::CryptoMethod { .. }
            | NativeCallable::CryptoDigestMethod { .. }
            | NativeCallable::StringPrimitiveMethod { .. }
            | NativeCallable::BufferStatic { .. }
            | NativeCallable::BufferMethod { .. }
            | NativeCallable::TextEncoderMethod { .. }
            | NativeCallable::TextDecoderMethod { .. }
            | NativeCallable::EvalFunction(_)
            | NativeCallable::PromiseResolvingFunction { .. }
            | NativeCallable::PromiseCombinatorReaction { .. }
            | NativeCallable::AsyncGeneratorMethod { .. }
            | NativeCallable::AsyncGeneratorIdentity { .. }
            | NativeCallable::GeneratorMethod { .. }
            | NativeCallable::GeneratorIdentity { .. }
            | NativeCallable::ArrayLikeIteratorNext { .. }
            | NativeCallable::RawIteratorNext { .. }
            | NativeCallable::AsyncFromSyncNext { .. }
            | NativeCallable::AsyncFromSyncReturn { .. }
            | NativeCallable::AsyncFromSyncThrow { .. }
            | NativeCallable::MapSetMethod { .. }
            | NativeCallable::DateMethod { .. }
            | NativeCallable::WeakMapMethod { .. }
            | NativeCallable::WeakSetMethod { .. }
            | NativeCallable::WeakRefDerefMethod
            | NativeCallable::FinalizationRegistryRegisterMethod
            | NativeCallable::FinalizationRegistryUnregisterMethod
            | NativeCallable::ProxyRevoker { .. }
            | NativeCallable::ProcessCwd
            | NativeCallable::ProcessExit
            | NativeCallable::ProcessNextTick
            | NativeCallable::ProcessSend
            | NativeCallable::ProcessDisconnect
            | NativeCallable::ProcessOn
            | NativeCallable::ProcessStreamWrite { .. }
            | NativeCallable::ProcessEnvTrap { .. }
            | NativeCallable::ProcessStreamEnd { .. }
            | NativeCallable::ProcessStreamOn { .. }
            | NativeCallable::ProcessStdinResume
            | NativeCallable::ProcessHrtime
            | NativeCallable::ProcessHrtimeBigint
            | NativeCallable::ProcessMemoryUsage
            | NativeCallable::ProcessUptime
            | NativeCallable::ProcessCpuUsage
            | NativeCallable::PromiseFinallyAwait { .. }
            | NativeCallable::HeadersMethod { .. }
            | NativeCallable::ResponseMethod { .. }
            | NativeCallable::RequestMethod { .. }
            | NativeCallable::AbortControllerAbort { .. }
            | NativeCallable::ReadableStreamMethod { .. }
            | NativeCallable::ReadableStreamDefaultReaderMethod { .. }
            | NativeCallable::ReadableStreamDefaultControllerMethod { .. }
            | NativeCallable::ReadableStreamByobRequestMethod { .. }
            | NativeCallable::ReadableStreamAsyncIteratorNext { .. }
            | NativeCallable::ReadableStreamAsyncIteratorReturn { .. }
            | NativeCallable::ReadableStreamPipeToWriteFulfilled { .. }
            | NativeCallable::ReadableStreamPipeToWriteRejected { .. }
            | NativeCallable::RegExpStringIteratorNext { .. }
            | NativeCallable::RegExpStringIteratorSelf
            | NativeCallable::WritableStreamMethod { .. }
            | NativeCallable::WritableStreamDefaultWriterMethod { .. }
            | NativeCallable::WritableStreamDefaultControllerMethod { .. }
            | NativeCallable::TransformStreamMethod { .. }
            | NativeCallable::QueuingStrategySize { .. }
            | NativeCallable::ObjectStatic { .. }
            | NativeCallable::PromiseStatic { .. } => {
                bail!("SnapshotNativeCallable: unsupported runtime-state-carrying variant")
            }
        };
        Ok(result)
    }
}
