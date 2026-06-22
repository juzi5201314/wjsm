//! Maps between runtime `NativeCallable` and `wjsm_snapshot_format::SnapshotNativeCallable`.

use anyhow::{Result, bail};

use crate::types::NativeCallable;
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
            Self::ArrayProtoValues => NativeCallable::ArrayProtoValues,
            Self::ArrayConstructor => NativeCallable::ArrayConstructor,
            Self::ObjectConstructor => NativeCallable::ObjectConstructor,
            Self::ObjectProtoToString => NativeCallable::ObjectProtoToString,
            Self::ObjectProtoValueOf => NativeCallable::ObjectProtoValueOf,
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
            Self::ArgumentsStrictCalleeGetter => NativeCallable::ArgumentsStrictCalleeGetter,
            Self::TypedArrayConstructor => NativeCallable::TypedArrayConstructor(()),
        }
    }

    fn try_from_native_callable(nc: &NativeCallable) -> Result<Self> {
        let result = match nc {
            NativeCallable::EvalIndirect => Self::EvalIndirect,
            NativeCallable::AsyncIteratorProtoSymbolAsyncIterator => {
                Self::AsyncIteratorProtoSymbolAsyncIterator
            }
            NativeCallable::ArrayProtoValues => Self::ArrayProtoValues,
            NativeCallable::ArrayConstructor => Self::ArrayConstructor,
            NativeCallable::ObjectConstructor => Self::ObjectConstructor,
            NativeCallable::ObjectProtoToString => Self::ObjectProtoToString,
            NativeCallable::ObjectProtoValueOf => Self::ObjectProtoValueOf,
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
            NativeCallable::TypedArrayConstructor(()) => Self::TypedArrayConstructor,
            NativeCallable::EvalFunction(_)
            | NativeCallable::PromiseResolvingFunction { .. }
            | NativeCallable::PromiseCombinatorReaction { .. }
            | NativeCallable::AsyncGeneratorMethod { .. }
            | NativeCallable::AsyncGeneratorIdentity { .. }
            | NativeCallable::ArrayLikeIteratorNext { .. }
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
            | NativeCallable::WritableStreamMethod { .. }
            | NativeCallable::WritableStreamDefaultWriterMethod { .. }
            | NativeCallable::WritableStreamDefaultControllerMethod { .. }
            | NativeCallable::TransformStreamMethod { .. }
            | NativeCallable::QueuingStrategySize { .. } => {
                bail!("SnapshotNativeCallable: unsupported runtime-state-carrying variant")
            }
        };
        Ok(result)
    }
}
