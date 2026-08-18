//! Native startup snapshot 的确定性 binary schema 与严格校验。

use std::collections::BTreeSet;

use anyhow::{Context, Result, bail};
use sha2::{Digest, Sha256};

pub const SNAPSHOT_MAGIC: [u8; 8] = *b"WJSMNSP\0";
pub const SNAPSHOT_FORMAT_VERSION: u32 = 1;

const HEADER_BYTES: usize = 220;
const DIRECTORY_ENTRY_BYTES: usize = 52;
const CONTENT_HASH_OFFSET: usize = 16;
const CONTENT_HASH_END: usize = 48;
const SECTION_TARGET: u16 = 1;
const SECTION_OBJECT_BYTES: u16 = 2;
const SECTION_HANDLES: u16 = 3;
const SECTION_SHAPES: u16 = 4;
const SECTION_HOST_STATE: u16 = 5;
const SECTION_COUNT: u32 = 5;
const HANDLE_BYTES: usize = 13;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum SnapshotEndian {
    Little = 1,
    Big = 2,
}

impl SnapshotEndian {
    pub const fn current() -> Self {
        if cfg!(target_endian = "little") {
            Self::Little
        } else {
            Self::Big
        }
    }

    fn decode(raw: u8) -> Result<Self> {
        match raw {
            1 => Ok(Self::Little),
            2 => Ok(Self::Big),
            _ => bail!("native snapshot has invalid endian tag {raw}"),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum SnapshotGeneration {
    Young = 0,
    Old = 1,
}

impl SnapshotGeneration {
    fn decode(raw: u8) -> Result<Self> {
        match raw {
            0 => Ok(Self::Young),
            1 => Ok(Self::Old),
            _ => bail!("native snapshot has invalid handle generation {raw}"),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SnapshotHandle {
    pub handle: u32,
    pub address: u64,
    pub generation: SnapshotGeneration,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NativeStartupSnapshot {
    pub bootstrap_hash: [u8; 32],
    pub lowering_hash: [u8; 32],
    pub semantic_abi_hash: [u8; 32],
    pub native_abi_hash: [u8; 32],
    pub target: String,
    pub endian: SnapshotEndian,
    pub object_heap_base: u64,
    pub object_heap_end: u64,
    pub next_handle: u64,
    pub global_object: i64,
    pub object_bytes: Vec<u8>,
    pub handles: Vec<SnapshotHandle>,
    pub shape_table_bytes: Vec<u8>,
    pub host_state_bytes: Vec<u8>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SnapshotExpectations<'a> {
    pub bootstrap_hash: [u8; 32],
    pub lowering_hash: [u8; 32],
    pub semantic_abi_hash: [u8; 32],
    pub native_abi_hash: [u8; 32],
    pub target: &'a str,
    pub endian: SnapshotEndian,
    pub object_heap_base: u64,
    pub object_heap_capacity_end: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SnapshotLimits {
    pub max_total_bytes: u64,
    pub max_section_bytes: u64,
    pub max_handles: u32,
    pub max_target_bytes: u32,
}

impl Default for SnapshotLimits {
    fn default() -> Self {
        Self {
            max_total_bytes: 256 * 1024 * 1024,
            max_section_bytes: 128 * 1024 * 1024,
            max_handles: 16 * 1024 * 1024,
            max_target_bytes: 4096,
        }
    }
}

pub fn encode_snapshot(snapshot: &NativeStartupSnapshot) -> Result<Vec<u8>> {
    validate_snapshot(snapshot, &SnapshotLimits::default())?;
    let sections = encoded_sections(snapshot)?;
    let directory_bytes = DIRECTORY_ENTRY_BYTES
        .checked_mul(sections.len())
        .context("native snapshot directory size overflows")?;
    let payload_start = HEADER_BYTES
        .checked_add(directory_bytes)
        .context("native snapshot payload offset overflows")?;
    let payload_bytes = sections.iter().try_fold(0_usize, |total, section| {
        total
            .checked_add(section.bytes.len())
            .context("native snapshot payload size overflows")
    })?;
    let total_bytes = payload_start
        .checked_add(payload_bytes)
        .context("native snapshot size overflows")?;
    let mut bytes = Vec::with_capacity(total_bytes);
    encode_header(snapshot, &mut bytes);
    let mut offset = payload_start;
    for section in &sections {
        bytes.extend_from_slice(&section.id.to_le_bytes());
        bytes.extend_from_slice(&0_u16.to_le_bytes());
        bytes.extend_from_slice(&u64::try_from(offset)?.to_le_bytes());
        bytes.extend_from_slice(&u64::try_from(section.bytes.len())?.to_le_bytes());
        bytes.extend_from_slice(&digest(&section.bytes));
        offset = offset
            .checked_add(section.bytes.len())
            .context("native snapshot section offset overflows")?;
    }
    for section in sections {
        bytes.extend_from_slice(&section.bytes);
    }
    debug_assert_eq!(bytes.len(), total_bytes);
    let content_hash = digest_with_zeroed_content_hash(&bytes);
    bytes[CONTENT_HASH_OFFSET..CONTENT_HASH_END].copy_from_slice(&content_hash);
    Ok(bytes)
}

pub fn decode_snapshot(
    bytes: &[u8],
    limits: &SnapshotLimits,
    expected: &SnapshotExpectations<'_>,
) -> Result<NativeStartupSnapshot> {
    if u64::try_from(bytes.len())? > limits.max_total_bytes {
        bail!("native snapshot exceeds total byte limit")
    }
    let header = bytes
        .get(..HEADER_BYTES)
        .context("native snapshot is truncated")?;
    if header[..8] != SNAPSHOT_MAGIC {
        bail!("native snapshot magic mismatch")
    }
    let mut reader = Reader::new(&header[8..]);
    if reader.u32()? != SNAPSHOT_FORMAT_VERSION {
        bail!("native snapshot format version mismatch")
    }
    if usize::try_from(reader.u32()?)? != HEADER_BYTES {
        bail!("native snapshot header size mismatch")
    }
    let content_hash = reader.array::<32>()?;
    if content_hash != digest_with_zeroed_content_hash(bytes) {
        bail!("native snapshot content hash mismatch")
    }
    if reader.u32()? != SECTION_COUNT {
        bail!("native snapshot section count mismatch")
    }
    let bootstrap_hash = reader.array::<32>()?;
    let lowering_hash = reader.array::<32>()?;
    let semantic_abi_hash = reader.array::<32>()?;
    let native_abi_hash = reader.array::<32>()?;
    let endian = SnapshotEndian::decode(reader.u8()?)?;
    reader.skip(7)?;
    let object_heap_base = reader.u64()?;
    let object_heap_end = reader.u64()?;
    let next_handle = reader.u64()?;
    let global_object = reader.i64()?;
    reader.finish()?;
    require_eq("bootstrap hash", bootstrap_hash, expected.bootstrap_hash)?;
    require_eq("lowering hash", lowering_hash, expected.lowering_hash)?;
    require_eq(
        "semantic ABI hash",
        semantic_abi_hash,
        expected.semantic_abi_hash,
    )?;
    require_eq("native ABI hash", native_abi_hash, expected.native_abi_hash)?;
    require_eq("endian", endian, expected.endian)?;
    require_eq(
        "object heap base",
        object_heap_base,
        expected.object_heap_base,
    )?;

    let sections = decode_directory(bytes, limits)?;
    let target = std::str::from_utf8(section(bytes, &sections, SECTION_TARGET)?)
        .context("native snapshot target is not UTF-8")?
        .to_owned();
    require_eq("target", target.as_str(), expected.target)?;
    if target.len() > limits.max_target_bytes as usize {
        bail!("native snapshot target exceeds byte limit")
    }
    let object_bytes = section(bytes, &sections, SECTION_OBJECT_BYTES)?.to_vec();
    let restored_object_end = object_heap_base
        .checked_add(u64::try_from(object_bytes.len())?)
        .context("native snapshot restored object range overflows")?;
    if restored_object_end > expected.object_heap_capacity_end {
        bail!("native snapshot objects exceed runtime heap capacity")
    }
    let handles = decode_handles(
        section(bytes, &sections, SECTION_HANDLES)?,
        limits.max_handles,
    )?;
    let shape_table_bytes = section(bytes, &sections, SECTION_SHAPES)?.to_vec();
    let host_state_bytes = section(bytes, &sections, SECTION_HOST_STATE)?.to_vec();
    let snapshot = NativeStartupSnapshot {
        bootstrap_hash,
        lowering_hash,
        semantic_abi_hash,
        native_abi_hash,
        target,
        endian,
        object_heap_base,
        object_heap_end,
        next_handle,
        global_object,
        object_bytes,
        handles,
        shape_table_bytes,
        host_state_bytes,
    };
    validate_snapshot(&snapshot, limits)?;
    Ok(snapshot)
}

pub fn snapshot_abi_hash() -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(SNAPSHOT_MAGIC);
    hasher.update(SNAPSHOT_FORMAT_VERSION.to_le_bytes());
    hasher.update((HEADER_BYTES as u64).to_le_bytes());
    hasher.update((DIRECTORY_ENTRY_BYTES as u64).to_le_bytes());
    hasher.update((HANDLE_BYTES as u64).to_le_bytes());
    for section in [
        SECTION_TARGET,
        SECTION_OBJECT_BYTES,
        SECTION_HANDLES,
        SECTION_SHAPES,
        SECTION_HOST_STATE,
    ] {
        hasher.update(section.to_le_bytes());
    }
    hasher.finalize().into()
}

fn validate_snapshot(snapshot: &NativeStartupSnapshot, limits: &SnapshotLimits) -> Result<()> {
    if snapshot.target.is_empty() {
        bail!("native snapshot target is empty")
    }
    if snapshot.target.len() > limits.max_target_bytes as usize {
        bail!("native snapshot target exceeds byte limit")
    }
    if snapshot.object_heap_base & 7 != 0 || snapshot.object_heap_end < snapshot.object_heap_base {
        bail!("native snapshot object heap layout is invalid")
    }
    let capacity = snapshot.object_heap_end - snapshot.object_heap_base;
    if u64::try_from(snapshot.object_bytes.len())? > capacity {
        bail!("native snapshot object bytes exceed heap layout")
    }
    if snapshot.next_handle > u64::from(u32::MAX) + 1 {
        bail!("native snapshot next handle exceeds u32 space")
    }
    if snapshot.handles.len() > limits.max_handles as usize {
        bail!("native snapshot handle count exceeds limit")
    }
    for bytes in [
        snapshot.object_bytes.as_slice(),
        snapshot.shape_table_bytes.as_slice(),
        snapshot.host_state_bytes.as_slice(),
    ] {
        if u64::try_from(bytes.len())? > limits.max_section_bytes {
            bail!("native snapshot section exceeds byte limit")
        }
    }
    let captured_end = snapshot
        .object_heap_base
        .checked_add(u64::try_from(snapshot.object_bytes.len())?)
        .context("native snapshot object range overflows")?;
    let mut previous = None;
    for handle in &snapshot.handles {
        if previous.is_some_and(|previous| previous >= handle.handle) {
            bail!("native snapshot handles are not strictly sorted")
        }
        if u64::from(handle.handle) >= snapshot.next_handle {
            bail!("native snapshot handle is outside next_handle")
        }
        if handle.address & 7 != 0
            || handle.address < snapshot.object_heap_base
            || handle.address >= captured_end
        {
            bail!("native snapshot handle address is outside captured objects")
        }
        previous = Some(handle.handle);
    }
    Ok(())
}

struct EncodedSection {
    id: u16,
    bytes: Vec<u8>,
}

fn encoded_sections(snapshot: &NativeStartupSnapshot) -> Result<Vec<EncodedSection>> {
    let mut handles = Vec::with_capacity(
        snapshot
            .handles
            .len()
            .checked_mul(HANDLE_BYTES)
            .context("native snapshot handle payload overflows")?,
    );
    for handle in &snapshot.handles {
        handles.extend_from_slice(&handle.handle.to_le_bytes());
        handles.extend_from_slice(&handle.address.to_le_bytes());
        handles.push(handle.generation as u8);
    }
    Ok(vec![
        EncodedSection {
            id: SECTION_TARGET,
            bytes: snapshot.target.as_bytes().to_vec(),
        },
        EncodedSection {
            id: SECTION_OBJECT_BYTES,
            bytes: snapshot.object_bytes.clone(),
        },
        EncodedSection {
            id: SECTION_HANDLES,
            bytes: handles,
        },
        EncodedSection {
            id: SECTION_SHAPES,
            bytes: snapshot.shape_table_bytes.clone(),
        },
        EncodedSection {
            id: SECTION_HOST_STATE,
            bytes: snapshot.host_state_bytes.clone(),
        },
    ])
}

fn encode_header(snapshot: &NativeStartupSnapshot, bytes: &mut Vec<u8>) {
    bytes.extend_from_slice(&SNAPSHOT_MAGIC);
    bytes.extend_from_slice(&SNAPSHOT_FORMAT_VERSION.to_le_bytes());
    bytes.extend_from_slice(&(HEADER_BYTES as u32).to_le_bytes());
    bytes.extend_from_slice(&[0; 32]);
    bytes.extend_from_slice(&SECTION_COUNT.to_le_bytes());
    bytes.extend_from_slice(&snapshot.bootstrap_hash);
    bytes.extend_from_slice(&snapshot.lowering_hash);
    bytes.extend_from_slice(&snapshot.semantic_abi_hash);
    bytes.extend_from_slice(&snapshot.native_abi_hash);
    bytes.push(snapshot.endian as u8);
    bytes.extend_from_slice(&[0; 7]);
    bytes.extend_from_slice(&snapshot.object_heap_base.to_le_bytes());
    bytes.extend_from_slice(&snapshot.object_heap_end.to_le_bytes());
    bytes.extend_from_slice(&snapshot.next_handle.to_le_bytes());
    bytes.extend_from_slice(&snapshot.global_object.to_le_bytes());
    debug_assert_eq!(bytes.len(), HEADER_BYTES);
}

#[derive(Clone, Copy)]
struct Section {
    offset: usize,
    len: usize,
}

fn decode_directory(bytes: &[u8], limits: &SnapshotLimits) -> Result<Vec<(u16, Section)>> {
    let directory_end = HEADER_BYTES
        .checked_add(DIRECTORY_ENTRY_BYTES * SECTION_COUNT as usize)
        .context("native snapshot directory offset overflows")?;
    let directory = bytes
        .get(HEADER_BYTES..directory_end)
        .context("native snapshot directory is truncated")?;
    let mut expected_offset = directory_end;
    let mut previous_id = None;
    let mut sections = Vec::with_capacity(SECTION_COUNT as usize);
    for entry in directory.as_chunks::<DIRECTORY_ENTRY_BYTES>().0 {
        let mut reader = Reader::new(entry);
        let id = reader.u16()?;
        if reader.u16()? != 0 {
            bail!("native snapshot section flags are non-canonical")
        }
        if previous_id.is_some_and(|previous| previous >= id) {
            bail!("native snapshot section IDs are not strictly sorted")
        }
        let offset = usize::try_from(reader.u64()?)?;
        let len = usize::try_from(reader.u64()?)?;
        let section_hash = reader.array::<32>()?;
        reader.finish()?;
        if offset != expected_offset {
            bail!("native snapshot sections contain a gap or overlap")
        }
        if u64::try_from(len)? > limits.max_section_bytes {
            bail!("native snapshot section exceeds byte limit")
        }
        let end = offset
            .checked_add(len)
            .context("native snapshot section range overflows")?;
        let payload = bytes
            .get(offset..end)
            .context("native snapshot section is truncated")?;
        if digest(payload) != section_hash {
            bail!("native snapshot section hash mismatch")
        }
        sections.push((id, Section { offset, len }));
        previous_id = Some(id);
        expected_offset = end;
    }
    if expected_offset != bytes.len() {
        bail!("native snapshot has trailing bytes")
    }
    let actual = sections.iter().map(|(id, _)| *id).collect::<BTreeSet<_>>();
    let expected = BTreeSet::from([
        SECTION_TARGET,
        SECTION_OBJECT_BYTES,
        SECTION_HANDLES,
        SECTION_SHAPES,
        SECTION_HOST_STATE,
    ]);
    if actual != expected {
        bail!("native snapshot required sections mismatch")
    }
    Ok(sections)
}

fn section<'a>(bytes: &'a [u8], sections: &[(u16, Section)], id: u16) -> Result<&'a [u8]> {
    let section = sections
        .iter()
        .find_map(|(candidate, section)| (*candidate == id).then_some(*section))
        .context("native snapshot required section is missing")?;
    Ok(&bytes[section.offset..section.offset + section.len])
}

fn decode_handles(bytes: &[u8], maximum: u32) -> Result<Vec<SnapshotHandle>> {
    if !bytes.len().is_multiple_of(HANDLE_BYTES) {
        bail!("native snapshot handle section length is invalid")
    }
    let count = bytes.len() / HANDLE_BYTES;
    if count > maximum as usize {
        bail!("native snapshot handle count exceeds limit")
    }
    let mut handles = Vec::with_capacity(count);
    for entry in bytes.as_chunks::<HANDLE_BYTES>().0 {
        handles.push(SnapshotHandle {
            handle: u32::from_le_bytes(entry[..4].try_into()?),
            address: u64::from_le_bytes(entry[4..12].try_into()?),
            generation: SnapshotGeneration::decode(entry[12])?,
        });
    }
    Ok(handles)
}

fn require_eq<T>(name: &str, actual: T, expected: T) -> Result<()>
where
    T: Eq,
{
    if actual != expected {
        bail!("native snapshot {name} mismatch")
    }
    Ok(())
}

fn digest(bytes: &[u8]) -> [u8; 32] {
    Sha256::digest(bytes).into()
}

fn digest_with_zeroed_content_hash(bytes: &[u8]) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(&bytes[..CONTENT_HASH_OFFSET]);
    hasher.update([0; 32]);
    hasher.update(&bytes[CONTENT_HASH_END..]);
    hasher.finalize().into()
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

    fn u64(&mut self) -> Result<u64> {
        Ok(u64::from_le_bytes(self.take(8)?.try_into()?))
    }

    fn i64(&mut self) -> Result<i64> {
        Ok(i64::from_le_bytes(self.take(8)?.try_into()?))
    }

    fn array<const N: usize>(&mut self) -> Result<[u8; N]> {
        Ok(self.take(N)?.try_into()?)
    }

    fn skip(&mut self, len: usize) -> Result<()> {
        self.take(len).map(|_| ())
    }

    fn take(&mut self, len: usize) -> Result<&'a [u8]> {
        let end = self
            .offset
            .checked_add(len)
            .context("native snapshot reader offset overflows")?;
        let bytes = self
            .bytes
            .get(self.offset..end)
            .context("native snapshot is truncated")?;
        self.offset = end;
        Ok(bytes)
    }

    fn finish(self) -> Result<()> {
        if self.offset != self.bytes.len() {
            bail!("native snapshot field has trailing bytes")
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snapshot() -> NativeStartupSnapshot {
        NativeStartupSnapshot {
            bootstrap_hash: [1; 32],
            lowering_hash: [2; 32],
            semantic_abi_hash: [3; 32],
            native_abi_hash: [4; 32],
            target: "x86_64-unknown-linux-gnu".into(),
            endian: SnapshotEndian::Little,
            object_heap_base: 0x8_0001_0000,
            object_heap_end: 0x8_0011_0000,
            next_handle: 2,
            global_object: -1,
            object_bytes: vec![0; 64],
            handles: vec![
                SnapshotHandle {
                    handle: 0,
                    address: 0x8_0001_0000,
                    generation: SnapshotGeneration::Young,
                },
                SnapshotHandle {
                    handle: 1,
                    address: 0x8_0001_0020,
                    generation: SnapshotGeneration::Old,
                },
            ],
            shape_table_bytes: vec![1, 2, 3],
            host_state_bytes: vec![4, 5, 6],
        }
    }

    fn expectations(snapshot: &NativeStartupSnapshot) -> SnapshotExpectations<'_> {
        SnapshotExpectations {
            bootstrap_hash: snapshot.bootstrap_hash,
            lowering_hash: snapshot.lowering_hash,
            semantic_abi_hash: snapshot.semantic_abi_hash,
            native_abi_hash: snapshot.native_abi_hash,
            target: &snapshot.target,
            endian: snapshot.endian,
            object_heap_base: snapshot.object_heap_base,
            object_heap_capacity_end: snapshot.object_heap_end,
        }
    }

    #[test]
    fn native_snapshot_roundtrips_all_owner_state() {
        let snapshot = snapshot();
        let bytes = encode_snapshot(&snapshot).expect("snapshot encodes");
        let decoded = decode_snapshot(&bytes, &SnapshotLimits::default(), &expectations(&snapshot))
            .expect("snapshot decodes");
        assert_eq!(decoded, snapshot);
    }

    #[test]
    fn native_snapshot_rejects_hash_target_and_base_drift() {
        let snapshot = snapshot();
        let bytes = encode_snapshot(&snapshot).expect("snapshot encodes");
        let mut wrong = expectations(&snapshot);
        wrong.bootstrap_hash = [9; 32];
        assert!(decode_snapshot(&bytes, &SnapshotLimits::default(), &wrong).is_err());
        let mut wrong = expectations(&snapshot);
        wrong.target = "aarch64-unknown-linux-gnu";
        assert!(decode_snapshot(&bytes, &SnapshotLimits::default(), &wrong).is_err());
        let mut wrong = expectations(&snapshot);
        wrong.object_heap_base += 8;
        assert!(decode_snapshot(&bytes, &SnapshotLimits::default(), &wrong).is_err());
    }

    #[test]
    fn native_snapshot_accepts_runtime_capacity_larger_than_captured_heap() {
        let snapshot = snapshot();
        let bytes = encode_snapshot(&snapshot).expect("snapshot encodes");
        let mut larger = expectations(&snapshot);
        larger.object_heap_capacity_end += 8;
        decode_snapshot(&bytes, &SnapshotLimits::default(), &larger)
            .expect("larger runtime heap capacity should accept snapshot");
    }

    #[test]
    fn native_snapshot_rejects_runtime_capacity_smaller_than_object_payload() {
        let snapshot = snapshot();
        let bytes = encode_snapshot(&snapshot).expect("snapshot encodes");
        let mut smaller = expectations(&snapshot);
        smaller.object_heap_capacity_end = snapshot.object_heap_base
            + u64::try_from(snapshot.object_bytes.len()).expect("fixture length fits u64")
            - 1;
        assert!(decode_snapshot(&bytes, &SnapshotLimits::default(), &smaller).is_err());
    }

    #[test]
    fn native_snapshot_rejects_corruption_and_noncanonical_handles() {
        let snapshot = snapshot();
        let mut bytes = encode_snapshot(&snapshot).expect("snapshot encodes");
        *bytes.last_mut().expect("snapshot is non-empty") ^= 1;
        assert!(
            decode_snapshot(&bytes, &SnapshotLimits::default(), &expectations(&snapshot),).is_err()
        );

        let mut invalid = snapshot.clone();
        invalid.handles.reverse();
        assert!(encode_snapshot(&invalid).is_err());
    }
}
