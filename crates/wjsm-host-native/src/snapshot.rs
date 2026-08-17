use std::collections::HashSet;
use std::sync::Arc;

use anyhow::{Context, Result, bail};
use wjsm_gc::heap::{HandleGeneration, HandleId, RestoredHandleEntry};
use wjsm_gc::{HeapAccessV2, NativeHeapMemory, ShapeTableSnapshot};
use wjsm_host::RuntimeString;
use wjsm_snapshot_format::{
    NativeStartupSnapshot, SnapshotEndian, SnapshotExpectations, SnapshotGeneration,
    SnapshotLimits, decode_snapshot,
};

use crate::{NativeAgentState, NativeCallableKind, NativeRuntimeError, gc};

include!(concat!(env!("OUT_DIR"), "/bootstrap_hash.rs"));

pub(crate) const STARTUP_SNAPSHOT_BYTES: &[u8] =
    include_bytes!(concat!(env!("OUT_DIR"), "/startup_snapshot.bin"));

const HOST_STATE_MAGIC: [u8; 8] = *b"WJSMHST\0";
const HOST_STATE_VERSION: u32 = 1;
const MAX_BOOTSTRAP_STRINGS: u32 = 1_000_000;
const MAX_BOOTSTRAP_STRING_UNITS: u32 = 16 * 1024 * 1024;

struct RestoredBootstrap {
    heap: Arc<HeapAccessV2<NativeHeapMemory>>,
    global_object: i64,
    strings: Vec<RuntimeString>,
    native_callables: Vec<NativeCallableKind>,
}

impl NativeAgentState {
    /// 恢复构建期嵌入的启动种子。`NativeRuntime::new_*` 必须调用；无关闭开关。
    pub(super) fn restore_startup_snapshot(
        &mut self,
        bytes: &[u8],
    ) -> Result<(), NativeRuntimeError> {
        let restored = restore(
            bytes,
            self.runtime_config.gc_algorithm,
            self.runtime_config.max_heap_size,
        )?;
        self.reset_execution();
        self.gc.reset_heap(restored.heap)?;
        self.gc.reset_nlab();
        self.global_object = Some(restored.global_object);
        self.strings = restored.strings;
        self.string_ids = self
            .strings
            .iter()
            .cloned()
            .enumerate()
            .map(|(index, string)| (string, index as u32))
            .collect();
        self.native_callables = restored.native_callables;
        self.native_callable_ids = self
            .native_callables
            .iter()
            .copied()
            .enumerate()
            .map(|(index, callable)| (callable, index as u32))
            .collect();
        self.ensure_intrinsic_prototypes()?;
        self.prepare_out_of_memory_error()?;
        let object_prototype = self
            .object_prototype
            .map(wjsm_ir::value::decode_handle)
            .ok_or_else(|| {
                NativeRuntimeError::Invariant(
                    "native startup snapshot object prototype is missing".into(),
                )
            })?;
        self.gc.heap().set_prototype(
            wjsm_ir::value::decode_object_handle(restored.global_object),
            object_prototype,
        )?;
        Ok(())
    }
}

fn restore(
    bytes: &[u8],
    algorithm: wjsm_gc::GcAlgorithmKind,
    max_heap_size: u64,
) -> Result<RestoredBootstrap, NativeRuntimeError> {
    let heap = gc::NativeGc::fresh_heap(algorithm, max_heap_size)?;
    let expected = SnapshotExpectations {
        bootstrap_hash: BOOTSTRAP_HASH,
        lowering_hash: wjsm_backend_native::NATIVE_CODEGEN_HASH,
        semantic_abi_hash: wjsm_artifact_format::semantic_abi_hash(),
        native_abi_hash: wjsm_native_abi::native_abi_hash(),
        target: &format!("{}-{}", std::env::consts::ARCH, std::env::consts::OS),
        endian: SnapshotEndian::current(),
        object_heap_base: heap.object_heap_base(),
        object_heap_capacity_end: heap.heap_limit_bytes(),
    };
    let snapshot =
        decode_snapshot(bytes, &SnapshotLimits::default(), &expected).map_err(snapshot_error)?;
    let shape_table: ShapeTableSnapshot = serde_json::from_slice(&snapshot.shape_table_bytes)
        .context("native snapshot shape table is invalid")
        .map_err(snapshot_error)?;
    let (strings, native_callables) = decode_host_state(&snapshot.host_state_bytes)
        .context("native snapshot host state is invalid")
        .map_err(snapshot_error)?;
    validate_global_object(&snapshot)?;
    let handles = snapshot
        .handles
        .iter()
        .map(|entry| RestoredHandleEntry {
            handle: HandleId::new(entry.handle),
            address: entry.address,
            generation: match entry.generation {
                SnapshotGeneration::Young => HandleGeneration::Young,
                SnapshotGeneration::Old => HandleGeneration::Old,
            },
        })
        .collect::<Vec<_>>();
    heap.restore_object_region(&snapshot.object_bytes)?;
    heap.restore_handles(&handles, snapshot.next_handle)?;
    heap.restore_page_metadata(&handles)?;
    heap.import_shapes(shape_table);
    Ok(RestoredBootstrap {
        heap,
        global_object: snapshot.global_object,
        strings,
        native_callables,
    })
}

fn validate_global_object(snapshot: &NativeStartupSnapshot) -> Result<(), NativeRuntimeError> {
    if !wjsm_ir::value::is_object(snapshot.global_object) {
        return Err(snapshot_error(anyhow::anyhow!(
            "native snapshot global value is not an object"
        )));
    }
    let global = wjsm_ir::value::decode_object_handle(snapshot.global_object);
    if !snapshot.handles.iter().any(|entry| entry.handle == global) {
        return Err(snapshot_error(anyhow::anyhow!(
            "native snapshot global object handle is missing"
        )));
    }
    Ok(())
}

fn decode_host_state(bytes: &[u8]) -> Result<(Vec<RuntimeString>, Vec<NativeCallableKind>)> {
    let mut reader = Reader::new(bytes);
    if reader.array::<8>()? != HOST_STATE_MAGIC {
        bail!("host state magic mismatch")
    }
    if reader.u32()? != HOST_STATE_VERSION {
        bail!("host state version mismatch")
    }
    let string_count = reader.u32()?;
    if string_count > MAX_BOOTSTRAP_STRINGS {
        bail!("host state string count exceeds limit")
    }
    let mut strings = Vec::with_capacity(string_count as usize);
    let mut unique = HashSet::with_capacity(string_count as usize);
    for _ in 0..string_count {
        let unit_count = reader.u32()?;
        if unit_count > MAX_BOOTSTRAP_STRING_UNITS {
            bail!("host state string length exceeds limit")
        }
        let mut units = Vec::with_capacity(unit_count as usize);
        for _ in 0..unit_count {
            units.push(reader.u16()?);
        }
        let string = RuntimeString::from_utf16_units(units);
        if !unique.insert(string.clone()) {
            bail!("host state contains duplicate interned string")
        }
        strings.push(string);
    }
    let callable_count = reader.u32()?;
    let mut native_callables = Vec::with_capacity(callable_count as usize);
    for _ in 0..callable_count {
        let builtin = wjsm_ir::Builtin::from_wire_id(reader.u16()?)
            .context("host state callable builtin is invalid")?;
        let with_receiver = match reader.u8()? {
            0 => false,
            1 => true,
            flag => bail!("host state callable receiver flag {flag} is invalid"),
        };
        native_callables.push(NativeCallableKind::Builtin(builtin, with_receiver));
    }
    reader.finish()?;
    let expected = vec![NativeCallableKind::Builtin(
        wjsm_ir::Builtin::EvalIndirect,
        false,
    )];
    if native_callables != expected {
        bail!("host state bootstrap callable registry mismatch")
    }
    Ok((strings, native_callables))
}

fn snapshot_error(error: anyhow::Error) -> NativeRuntimeError {
    NativeRuntimeError::Invariant(format!("native startup snapshot rejected: {error:#}"))
}

struct Reader<'a> {
    bytes: &'a [u8],
    offset: usize,
}

impl<'a> Reader<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, offset: 0 }
    }

    fn u8(&mut self) -> Result<u8> {
        Ok(self.take(1)?[0])
    }

    fn u16(&mut self) -> Result<u16> {
        Ok(u16::from_le_bytes(self.take(2)?.try_into()?))
    }

    fn u32(&mut self) -> Result<u32> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into()?))
    }

    fn array<const N: usize>(&mut self) -> Result<[u8; N]> {
        Ok(self.take(N)?.try_into()?)
    }

    fn take(&mut self, len: usize) -> Result<&'a [u8]> {
        let end = self
            .offset
            .checked_add(len)
            .context("host state reader offset overflows")?;
        let bytes = self
            .bytes
            .get(self.offset..end)
            .context("host state is truncated")?;
        self.offset = end;
        Ok(bytes)
    }

    fn finish(self) -> Result<()> {
        if self.offset != self.bytes.len() {
            bail!("host state has trailing bytes")
        }
        Ok(())
    }
}
