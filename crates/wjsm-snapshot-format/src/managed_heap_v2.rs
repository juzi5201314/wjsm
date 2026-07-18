use anyhow::{Result, bail};
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

const MAGIC: [u8; 8] = *b"WJSMHV2\0";
const FORMAT_VERSION: u32 = 1;
const HEADER_BYTES: usize = 60;
const PAGE_BYTES: usize = 32;
const HANDLE_BYTES: usize = 13;
const HANDLE_ENTRY_BYTES: u8 = 8;
const ARTIFACT_ABI_MAGIC: [u8; 8] = *b"WJSMHA2\0";
const ARTIFACT_ABI_VERSION: u32 = 1;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ManagedHeapV2Generation {
    Young,
    Old,
}

impl ManagedHeapV2Generation {
    fn encode(self) -> u8 {
        match self {
            Self::Young => 0,
            Self::Old => 1,
        }
    }

    fn decode(value: u8) -> Result<Self> {
        match value {
            0 => Ok(Self::Young),
            1 => Ok(Self::Old),
            _ => bail!("managed heap V2 snapshot has invalid handle generation {value}"),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ManagedHeapV2Layout {
    pub object_heap_base: u64,
    pub object_heap_end: u64,
    pub page_bytes: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ManagedHeapV2Page {
    pub page_id: u32,
    pub range_start: u64,
    pub range_end: u64,
    pub object_count: u32,
    pub current_marked: u32,
    pub previous_marked: u32,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ManagedHeapV2Handle {
    pub handle: u32,
    pub raw_entry: u64,
    pub generation: ManagedHeapV2Generation,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ManagedHeapV2Snapshot {
    pub engine_fingerprint: u64,
    pub layout: ManagedHeapV2Layout,
    pub pages: Vec<ManagedHeapV2Page>,
    pub handles: Vec<ManagedHeapV2Handle>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ManagedHeapV2ArtifactAbi {
    pub engine_fingerprint: u64,
    pub support_abi_hash: u64,
}

pub fn managed_heap_v2_snapshot_abi_hash() -> u64 {
    let mut hasher = DefaultHasher::new();
    super::abi_hash().hash(&mut hasher);
    MAGIC.hash(&mut hasher);
    FORMAT_VERSION.hash(&mut hasher);
    HANDLE_ENTRY_BYTES.hash(&mut hasher);
    PAGE_BYTES.hash(&mut hasher);
    HANDLE_BYTES.hash(&mut hasher);
    hasher.finish()
}

pub fn encode_managed_heap_v2_snapshot(snapshot: &ManagedHeapV2Snapshot) -> Result<Vec<u8>> {
    validate(snapshot)?;
    let page_count = u32::try_from(snapshot.pages.len())?;
    let handle_count = u32::try_from(snapshot.handles.len())?;
    let payload_bytes = snapshot
        .pages
        .len()
        .checked_mul(PAGE_BYTES)
        .and_then(|pages| {
            snapshot
                .handles
                .len()
                .checked_mul(HANDLE_BYTES)
                .and_then(|handles| pages.checked_add(handles))
        })
        .ok_or_else(|| anyhow::anyhow!("managed heap V2 snapshot payload size overflows usize"))?;
    let mut bytes = Vec::with_capacity(
        HEADER_BYTES
            .checked_add(payload_bytes)
            .ok_or_else(|| anyhow::anyhow!("managed heap V2 snapshot size overflows usize"))?,
    );
    bytes.extend_from_slice(&MAGIC);
    put_u32(&mut bytes, FORMAT_VERSION);
    put_u64(&mut bytes, managed_heap_v2_snapshot_abi_hash());
    put_u64(&mut bytes, snapshot.engine_fingerprint);
    put_u64(&mut bytes, snapshot.layout.object_heap_base);
    put_u64(&mut bytes, snapshot.layout.object_heap_end);
    put_u64(&mut bytes, snapshot.layout.page_bytes);
    put_u32(&mut bytes, page_count);
    put_u32(&mut bytes, handle_count);
    for page in &snapshot.pages {
        put_u32(&mut bytes, page.page_id);
        put_u64(&mut bytes, page.range_start);
        put_u64(&mut bytes, page.range_end);
        put_u32(&mut bytes, page.object_count);
        put_u32(&mut bytes, page.current_marked);
        put_u32(&mut bytes, page.previous_marked);
    }
    for handle in &snapshot.handles {
        put_u32(&mut bytes, handle.handle);
        put_u64(&mut bytes, handle.raw_entry);
        bytes.push(handle.generation.encode());
    }
    Ok(bytes)
}

pub fn decode_managed_heap_v2_snapshot(
    bytes: &[u8],
    expected_engine_fingerprint: u64,
) -> Result<ManagedHeapV2Snapshot> {
    let mut reader = Reader::new(bytes);
    if reader.read_array::<8>()? != MAGIC {
        bail!("managed heap V2 snapshot magic mismatch")
    }
    if reader.read_u32()? != FORMAT_VERSION {
        bail!("managed heap V2 snapshot format version mismatch")
    }
    if reader.read_u64()? != managed_heap_v2_snapshot_abi_hash() {
        bail!("managed heap V2 snapshot ABI fingerprint mismatch")
    }
    let engine_fingerprint = reader.read_u64()?;
    if engine_fingerprint != expected_engine_fingerprint {
        bail!("managed heap V2 snapshot engine fingerprint mismatch")
    }
    let layout = ManagedHeapV2Layout {
        object_heap_base: reader.read_u64()?,
        object_heap_end: reader.read_u64()?,
        page_bytes: reader.read_u64()?,
    };
    let page_count = usize::try_from(reader.read_u32()?)?;
    let handle_count = usize::try_from(reader.read_u32()?)?;
    reader.require_remaining(
        page_count
            .checked_mul(PAGE_BYTES)
            .and_then(|pages| {
                handle_count
                    .checked_mul(HANDLE_BYTES)
                    .and_then(|handles| pages.checked_add(handles))
            })
            .ok_or_else(|| anyhow::anyhow!("managed heap V2 snapshot entry count overflows"))?,
    )?;
    let mut pages = Vec::with_capacity(page_count);
    for _ in 0..page_count {
        pages.push(ManagedHeapV2Page {
            page_id: reader.read_u32()?,
            range_start: reader.read_u64()?,
            range_end: reader.read_u64()?,
            object_count: reader.read_u32()?,
            current_marked: reader.read_u32()?,
            previous_marked: reader.read_u32()?,
        });
    }
    let mut handles = Vec::with_capacity(handle_count);
    for _ in 0..handle_count {
        handles.push(ManagedHeapV2Handle {
            handle: reader.read_u32()?,
            raw_entry: reader.read_u64()?,
            generation: ManagedHeapV2Generation::decode(reader.read_u8()?)?,
        });
    }
    reader.finish()?;
    let snapshot = ManagedHeapV2Snapshot {
        engine_fingerprint,
        layout,
        pages,
        handles,
    };
    validate(&snapshot)?;
    Ok(snapshot)
}

pub fn encode_managed_heap_v2_artifact_abi(artifact: ManagedHeapV2ArtifactAbi) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(36);
    bytes.extend_from_slice(&ARTIFACT_ABI_MAGIC);
    put_u32(&mut bytes, ARTIFACT_ABI_VERSION);
    put_u64(&mut bytes, managed_heap_v2_snapshot_abi_hash());
    put_u64(&mut bytes, artifact.engine_fingerprint);
    put_u64(&mut bytes, artifact.support_abi_hash);
    bytes
}

pub fn decode_managed_heap_v2_artifact_abi(
    bytes: &[u8],
    expected_engine_fingerprint: u64,
    expected_support_abi_hash: u64,
) -> Result<ManagedHeapV2ArtifactAbi> {
    let mut reader = Reader::new(bytes);
    if reader.read_array::<8>()? != ARTIFACT_ABI_MAGIC {
        bail!("managed heap V2 artifact ABI magic mismatch")
    }
    if reader.read_u32()? != ARTIFACT_ABI_VERSION {
        bail!("managed heap V2 artifact ABI version mismatch")
    }
    if reader.read_u64()? != managed_heap_v2_snapshot_abi_hash() {
        bail!("managed heap V2 artifact snapshot ABI fingerprint mismatch")
    }
    let artifact = ManagedHeapV2ArtifactAbi {
        engine_fingerprint: reader.read_u64()?,
        support_abi_hash: reader.read_u64()?,
    };
    reader.finish()?;
    if artifact.engine_fingerprint != expected_engine_fingerprint {
        bail!("managed heap V2 artifact engine fingerprint mismatch")
    }
    if artifact.support_abi_hash != expected_support_abi_hash {
        bail!("managed heap V2 artifact support ABI fingerprint mismatch")
    }
    Ok(artifact)
}

fn validate(snapshot: &ManagedHeapV2Snapshot) -> Result<()> {
    let layout = &snapshot.layout;
    if layout.page_bytes == 0 {
        bail!("managed heap V2 snapshot page size is zero")
    }
    if layout.object_heap_base >= layout.object_heap_end {
        bail!("managed heap V2 snapshot object heap range is empty")
    }
    let mut previous_page_id = None;
    let mut previous_range_end = layout.object_heap_base;
    for page in &snapshot.pages {
        if previous_page_id.is_some_and(|previous| page.page_id <= previous) {
            bail!("managed heap V2 snapshot page IDs are not strictly increasing")
        }
        if page.range_start < layout.object_heap_base
            || page.range_end > layout.object_heap_end
            || page.range_start >= page.range_end
            || page.range_start < previous_range_end
        {
            bail!("managed heap V2 snapshot page range is invalid")
        }
        if (page.range_start - layout.object_heap_base) % layout.page_bytes != 0
            || (page.range_end - layout.object_heap_base) % layout.page_bytes != 0
        {
            bail!("managed heap V2 snapshot page range is not page-aligned")
        }
        if page.current_marked > page.object_count || page.previous_marked > page.object_count {
            bail!("managed heap V2 snapshot bitmap count exceeds page object count")
        }
        previous_page_id = Some(page.page_id);
        previous_range_end = page.range_end;
    }
    let mut previous_handle = None;
    for handle in &snapshot.handles {
        if previous_handle.is_some_and(|previous| handle.handle <= previous) {
            bail!("managed heap V2 snapshot handle IDs are not strictly increasing")
        }
        previous_handle = Some(handle.handle);
    }
    Ok(())
}

fn put_u32(bytes: &mut Vec<u8>, value: u32) {
    bytes.extend_from_slice(&value.to_le_bytes());
}

fn put_u64(bytes: &mut Vec<u8>, value: u64) {
    bytes.extend_from_slice(&value.to_le_bytes());
}

struct Reader<'a> {
    bytes: &'a [u8],
    offset: usize,
}

impl<'a> Reader<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, offset: 0 }
    }

    fn read_array<const N: usize>(&mut self) -> Result<[u8; N]> {
        let slice = self.take(N)?;
        slice
            .try_into()
            .map_err(|_| anyhow::anyhow!("managed heap V2 snapshot fixed read has wrong length"))
    }

    fn read_u8(&mut self) -> Result<u8> {
        Ok(self.take(1)?[0])
    }

    fn read_u32(&mut self) -> Result<u32> {
        Ok(u32::from_le_bytes(self.read_array()?))
    }

    fn read_u64(&mut self) -> Result<u64> {
        Ok(u64::from_le_bytes(self.read_array()?))
    }

    fn require_remaining(&self, len: usize) -> Result<()> {
        if len > self.bytes.len().saturating_sub(self.offset) {
            bail!("managed heap V2 snapshot is truncated")
        }
        Ok(())
    }

    fn take(&mut self, len: usize) -> Result<&'a [u8]> {
        self.require_remaining(len)?;
        let start = self.offset;
        self.offset += len;
        Ok(&self.bytes[start..self.offset])
    }

    fn finish(self) -> Result<()> {
        if self.offset != self.bytes.len() {
            bail!("managed heap V2 snapshot has trailing bytes")
        }
        Ok(())
    }
}
