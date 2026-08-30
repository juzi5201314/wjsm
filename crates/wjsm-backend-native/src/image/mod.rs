mod platform;

use std::collections::HashMap;
#[cfg(any(target_os = "linux", target_os = "macos"))]
use std::ffi::c_void;
use std::sync::Arc;

use object::{
    Object, ObjectSection, ObjectSymbol, RelocationFlags, RelocationKind, RelocationTarget,
    SectionIndex, SymbolSection,
};
use thiserror::Error;
#[cfg(all(target_os = "windows", target_arch = "aarch64"))]
use windows_sys::Win32::System::Diagnostics::Debug::IMAGE_ARM64_RUNTIME_FUNCTION_ENTRY as PlatformRuntimeFunction;
#[cfg(all(target_os = "windows", target_arch = "x86_64"))]
use windows_sys::Win32::System::Diagnostics::Debug::IMAGE_RUNTIME_FUNCTION_ENTRY as PlatformRuntimeFunction;
#[cfg(target_os = "windows")]
use windows_sys::Win32::System::Diagnostics::Debug::{RtlAddFunctionTable, RtlDeleteFunctionTable};
use wjsm_ir::constants;
use wjsm_native_abi::{NativeFeedbackSlot, NativeFunctionEntry, NativeHostSymbol, NativeSlowEntry};

use crate::{IcTemplateHint, NativeObject, NativeSymbolResolver};
use platform::{ExecutableMapping, align_to_page, page_size};

/// IC 缓冲上限：4M 槽 × 8 word = 32M u32 = 128 MiB，防御恶意/损坏的 cache 条目。
const MAX_IC_BUFFER_WORDS: usize = 4_000_000 * 8;
/// 反馈缓冲上限：4M 槽 × 80 字节 = 320 MiB，防御恶意/损坏的 cache 条目。
const MAX_FEEDBACK_BUFFER_BYTES: usize = 4_000_000 * 48;

fn prefill_template_ic_slots_slice(
    ic_slots: &mut [u32],
    hints: &[IcTemplateHint],
    object_template_meta: &[u32],
) {
    let words_per_slot = usize::try_from(constants::IC_SLOT_SIZE)
        .unwrap_or(32)
        .checked_div(4)
        .unwrap_or(8);
    for (slot_index, hint) in hints.iter().enumerate() {
        let Some(meta_index) = hint.template_meta_index else {
            continue;
        };
        let Ok(entry_start) = usize::try_from(meta_index) else {
            continue;
        };
        let Some(entry_start) =
            entry_start.checked_mul(constants::OBJECT_TEMPLATE_META_WORDS as usize)
        else {
            continue;
        };
        let Some(entry_end) =
            entry_start.checked_add(constants::OBJECT_TEMPLATE_META_WORDS as usize)
        else {
            continue;
        };
        let Some(meta) = object_template_meta.get(entry_start..entry_end) else {
            continue;
        };
        let Some(&shape_id) = meta.first() else {
            continue;
        };
        let Some(base) = slot_index.checked_mul(words_per_slot) else {
            continue;
        };
        let Some(end) = base.checked_add(words_per_slot) else {
            continue;
        };
        let Some(slot) = ic_slots.get_mut(base..end) else {
            continue;
        };
        if let Some(trio) = hint.trio_prop_indices {
            prefill_trio_slot(slot, meta, shape_id, trio);
            continue;
        }
        let Some(prop_index) = hint.prop_index else {
            continue;
        };
        let Ok(prop_index) = usize::try_from(prop_index) else {
            continue;
        };
        let Some(value_index) = meta.get(4 + prop_index).copied() else {
            continue;
        };
        slot[0] = shape_id;
        slot[1] = value_index;
        slot[2] = constants::IC_KIND_OWN_DATA;
        for word in slot.iter_mut().skip(3) {
            *word = 0;
        }
    }
}

fn prefill_trio_slot(slot: &mut [u32], meta: &[u32], shape_id: u32, trio: [u32; 3]) {
    let Some(name_index) = meta_prop_index(meta, trio[0]) else {
        mark_trio_site(slot);
        return;
    };
    let Some(value_index) = meta_prop_index(meta, trio[1]) else {
        mark_trio_site(slot);
        return;
    };
    let Some(length_index) = meta_prop_index(meta, trio[2]) else {
        mark_trio_site(slot);
        return;
    };
    slot[0] = shape_id;
    slot[1] = name_index;
    slot[2] = constants::IC_KIND_OWN_DATA_TRIO;
    slot[3] = 0;
    slot[4] = value_index;
    slot[5] = length_index;
    slot[6] = 0;
    slot[7] = 0;
}

fn meta_prop_index(meta: &[u32], prop_index: u32) -> Option<u32> {
    meta.get(4 + usize::try_from(prop_index).ok()?).copied()
}

fn mark_trio_site(slot: &mut [u32]) {
    slot.fill(0);
    if let Some(marker) = slot.get_mut(6) {
        *marker = constants::IC_SLOT_TRIO_SITE_MARKER;
    }
}

pub struct CompiledImage {
    image_id: u64,
    entries: Box<[NativeFunctionEntry]>,
    mapping: ExecutableMapping,
    unwind: Option<UnwindRegistration>,
    code_bytes: usize,
    rodata_bytes: usize,
    /// 每访问点 32 字节的 IC 槽区（零初始化 = Empty）；生成代码经 vmctx 的
    /// `ic_slots_base` 访问，miss 时由宿主回填。
    ic_slots: Box<[u32]>,
    /// 每调用点 80 字节的类型反馈槽区（零初始化 = Empty）；生成代码经 vmctx 的
    /// `feedback_slots_base` 访问。特化 overlay image 不分配该缓冲——它永不激活
    /// 为 current image，其生成代码经由 vmctx 继续写 base image 的反馈区。
    feedback_slots: Box<[NativeFeedbackSlot]>,
}

// SAFETY: `CompiledImage` 只包含发布后不可变的 RX/R 映射、typed entry 和 unwind token。
unsafe impl Send for CompiledImage {}
// SAFETY: 所有共享字段在构造完成后只读；Drop 由 Arc 的最后一个 owner 串行执行。
unsafe impl Sync for CompiledImage {}

impl CompiledImage {
    pub fn load(
        object: &NativeObject,
        image_id: u64,
        resolver: &dyn NativeSymbolResolver,
    ) -> Result<Arc<Self>, ImageLoadError> {
        let file = object::File::parse(object.bytes())?;
        validate_object(&file)?;
        let mut loaded = load_sections(&file, resolver)?;
        apply_relocations(&file, &mut loaded, resolver)?;
        patch_platform_unwind(&file, &mut loaded)?;
        finalize_sections(&loaded)?;
        let entries = build_entries(&file, &loaded, object, image_id)?;
        let unwind = Some(register_unwind(&loaded)?);
        let code_bytes = loaded
            .sections
            .iter()
            .filter(|section| section.executable)
            .map(|section| section.loaded_len)
            .sum();
        let rodata_bytes = loaded
            .sections
            .iter()
            .filter(|section| !section.executable)
            .map(|section| section.loaded_len)
            .sum();
        let ic_word_count = usize::try_from(object.ic_slot_count())
            .map_err(|_| ImageLoadError::InvalidIcSlotCount)?
            .checked_mul(usize::try_from(wjsm_ir::constants::IC_SLOT_SIZE).unwrap_or(usize::MAX))
            .ok_or(ImageLoadError::InvalidIcSlotCount)?
            .checked_div(4)
            .ok_or(ImageLoadError::InvalidIcSlotCount)?;
        if ic_word_count > MAX_IC_BUFFER_WORDS {
            return Err(ImageLoadError::InvalidIcSlotCount);
        }
        let ic_slots = vec![0_u32; ic_word_count].into_boxed_slice();
        let feedback_slot_count = usize::try_from(object.feedback_slot_count())
            .map_err(|_| ImageLoadError::InvalidFeedbackSlotCount)?;
        let feedback_bytes = feedback_slot_count
            .checked_mul(
                usize::try_from(wjsm_ir::constants::FEEDBACK_SLOT_SIZE).unwrap_or(usize::MAX),
            )
            .ok_or(ImageLoadError::InvalidFeedbackSlotCount)?;
        if feedback_bytes > MAX_FEEDBACK_BUFFER_BYTES {
            return Err(ImageLoadError::InvalidFeedbackSlotCount);
        }
        let feedback_slots =
            vec![NativeFeedbackSlot::default(); feedback_slot_count].into_boxed_slice();
        Ok(Arc::new(Self {
            image_id,
            entries: entries.into_boxed_slice(),
            mapping: loaded.mapping,
            unwind,
            code_bytes,
            rodata_bytes,
            ic_slots,
            feedback_slots,
        }))
    }

    /// 只加载一个导出 symbol 作为 entry 的 overlay 通道（运行时特化专用）。
    ///
    /// 与 [`Self::load`] 共享同一套 RW → relocation → RX、unwind 注册与 Drop
    /// 顺序，但：entry 的 `local_function_id` 是 base image 内的函数下标（调用
    /// 帧/栈回溯共享 base 身份），typed entry 不注册为 JS function table entry，
    /// 也不分配 IC/反馈缓冲（overlay 生成代码经 vmctx 继续使用 base 的区域）。
    /// overlay 永不写入 repository、磁盘 cache 或分发制品。
    pub fn load_single_entry(
        object: &NativeObject,
        image_id: u64,
        local_function_id: u32,
        exported_symbol: &str,
        resolver: &dyn NativeSymbolResolver,
    ) -> Result<Arc<Self>, ImageLoadError> {
        let file = object::File::parse(object.bytes())?;
        validate_object(&file)?;
        let mut loaded = load_sections(&file, resolver)?;
        apply_relocations(&file, &mut loaded, resolver)?;
        patch_platform_unwind(&file, &mut loaded)?;
        finalize_sections(&loaded)?;
        let symbol = file
            .symbol_by_name(exported_symbol)
            .ok_or_else(|| ImageLoadError::UnknownSymbol(exported_symbol.to_owned()))?;
        let SymbolSection::Section(section) = symbol.section() else {
            return Err(ImageLoadError::UnknownSymbol(exported_symbol.to_owned()));
        };
        let locations: HashMap<_, _> = loaded
            .sections
            .iter()
            .enumerate()
            .filter_map(|(position, section)| section.index.map(|index| (index, position)))
            .collect();
        let position = locations
            .get(&section)
            .copied()
            .ok_or_else(|| ImageLoadError::UnknownSymbol(exported_symbol.to_owned()))?;
        let address = loaded.sections[position].address(&loaded.mapping, symbol.address())?;
        let pointer = std::ptr::with_exposed_provenance::<u8>(address);
        // SAFETY: 与 build_entries 同一验证链——symbol 指向已验证、已重定位并最终
        // 设为 RX 的 slow-entry 函数，映射由返回的 CompiledImage 持有。
        let slow_entry = unsafe { std::mem::transmute::<*const u8, NativeSlowEntry>(pointer) };
        let frame_bytes = *object
            .frame_bytes()
            .first()
            .ok_or(ImageLoadError::AddressOverflow)?;
        let osr_addr =
            resolve_exported_symbol_address(&file, &loaded, "wjsm_specialized_body").unwrap_or(0);
        let entries = vec![NativeFunctionEntry {
            slow_entry,
            local_function_id,
            frame_bytes,
            image_id: 0,
            osr_entry: std::sync::atomic::AtomicUsize::new(osr_addr),
        }]
        .into_boxed_slice();
        let unwind = Some(register_unwind(&loaded)?);
        let code_bytes = loaded
            .sections
            .iter()
            .filter(|section| section.executable)
            .map(|section| section.loaded_len)
            .sum();
        let rodata_bytes = loaded
            .sections
            .iter()
            .filter(|section| !section.executable)
            .map(|section| section.loaded_len)
            .sum();
        Ok(Arc::new(Self {
            image_id,
            entries,
            mapping: loaded.mapping,
            unwind,
            code_bytes,
            rodata_bytes,
            ic_slots: Box::new([]),
            feedback_slots: Box::new([]),
        }))
    }

    pub fn image_id(&self) -> u64 {
        self.image_id
    }

    pub fn entries(&self) -> &[NativeFunctionEntry] {
        &self.entries
    }

    pub fn code_bytes(&self) -> usize {
        self.code_bytes
    }

    pub fn rodata_bytes(&self) -> usize {
        self.rodata_bytes
    }

    pub fn feedback_slot_count(&self) -> u32 {
        u32::try_from(self.feedback_slots.len()).unwrap_or(0)
    }

    /// 反馈区基址（16 字节/槽对齐到 16 字节）；无反馈槽的 image（含 overlay）为 null。
    pub fn feedback_slots(&self) -> *mut u8 {
        if self.feedback_slots.is_empty() {
            std::ptr::null_mut()
        } else {
            self.feedback_slots.as_ptr().cast::<u8>().cast_mut()
        }
    }

    /// IC 区基址（16 字节/槽对齐到 16 字节）；无 IC 槽的 image 返回 null。
    pub fn ic_slots(&self) -> *const u32 {
        if self.ic_slots.is_empty() {
            std::ptr::null()
        } else {
            self.ic_slots.as_ptr()
        }
    }

    /// IC 区 u32 字数（每槽 8 字）；无 IC 槽时为 0。
    pub fn ic_slot_word_count(&self) -> usize {
        self.ic_slots.len()
    }

    /// install 期按编译 hint 与对象模板烘焙元数据预填 IC 槽（OWN_DATA）。
    pub fn prefill_template_ic_slots(
        &mut self,
        hints: &[IcTemplateHint],
        object_template_meta: &[u32],
    ) {
        prefill_template_ic_slots_slice(&mut self.ic_slots, hints, object_template_meta);
    }
}

impl Drop for CompiledImage {
    fn drop(&mut self) {
        self.unwind.take();
        self.entries = Box::new([]);
        let _ = &self.mapping;
    }
}

struct LoadedImage {
    mapping: ExecutableMapping,
    sections: Vec<LoadedSection>,
}

struct LoadedSection {
    index: Option<SectionIndex>,
    name: String,
    mapping_offset: usize,
    mapping_len: usize,
    data_len: usize,
    loaded_len: usize,
    executable: bool,
    call_stubs: HashMap<RelocationKey, u64>,
    got_slots: HashMap<RelocationKey, u64>,
}

impl LoadedSection {
    fn address(&self, mapping: &ExecutableMapping, offset: u64) -> Result<usize, ImageLoadError> {
        let offset = usize::try_from(offset).map_err(|_| ImageLoadError::AddressOverflow)?;
        if offset > self.loaded_len {
            return Err(ImageLoadError::SectionOutOfBounds);
        }
        mapping
            .address()
            .checked_add(self.mapping_offset)
            .and_then(|address| address.checked_add(offset))
            .ok_or(ImageLoadError::AddressOverflow)
    }
}

fn validate_object(file: &object::File<'_>) -> Result<(), ImageLoadError> {
    if !file.is_64() || !file.is_little_endian() {
        return Err(ImageLoadError::UnsupportedObject(
            "native image must be 64-bit little-endian".into(),
        ));
    }
    let expected = if cfg!(target_arch = "x86_64") {
        object::Architecture::X86_64
    } else {
        object::Architecture::Aarch64
    };
    if file.architecture() != expected {
        return Err(ImageLoadError::UnsupportedObject(format!(
            "object architecture {:?} does not match host {expected:?}",
            file.architecture()
        )));
    }
    let expected_format = if cfg!(target_os = "linux") {
        object::BinaryFormat::Elf
    } else if cfg!(target_os = "macos") {
        object::BinaryFormat::MachO
    } else {
        object::BinaryFormat::Coff
    };
    if file.format() != expected_format {
        return Err(ImageLoadError::UnsupportedObject(format!(
            "object format {:?} does not match host {expected_format:?}",
            file.format()
        )));
    }
    Ok(())
}

fn load_sections(
    file: &object::File<'_>,
    resolver: &dyn NativeSymbolResolver,
) -> Result<LoadedImage, ImageLoadError> {
    struct PreparedSection {
        index: SectionIndex,
        name: String,
        data_len: usize,
        loaded_len: usize,
        executable: bool,
        call_targets: Vec<(RelocationKey, usize)>,
        got_keys: Vec<RelocationKey>,
    }

    let mut prepared = Vec::new();
    for section in file.sections() {
        let name = section.name()?.to_owned();
        let data = section.data()?;
        if data.is_empty() || is_ignored_section(&name) {
            continue;
        }
        let executable = is_code_section(&name);
        if !executable && !is_read_only_section(&name) {
            return Err(ImageLoadError::ForbiddenSection(name));
        }
        let call_targets = if executable {
            external_call_targets(file, section.index(), resolver)?
        } else {
            Vec::new()
        };
        let got_keys = local_got_keys(file, section.index())?;
        let call_bytes = call_targets
            .len()
            .checked_mul(call_stub_size())
            .ok_or(ImageLoadError::AddressOverflow)?;
        let call_end = data
            .len()
            .checked_add(call_bytes)
            .ok_or(ImageLoadError::AddressOverflow)?;
        let got_base = call_end
            .checked_add(7)
            .map(|offset| offset & !7)
            .ok_or(ImageLoadError::AddressOverflow)?;
        let got_bytes = got_keys
            .len()
            .checked_mul(size_of::<u64>())
            .ok_or(ImageLoadError::AddressOverflow)?;
        let loaded_len = got_base
            .checked_add(got_bytes)
            .ok_or(ImageLoadError::AddressOverflow)?;
        prepared.push(PreparedSection {
            index: section.index(),
            name,
            data_len: data.len(),
            loaded_len,
            executable,
            call_targets,
            got_keys,
        });
    }
    if !prepared.iter().any(|section| section.executable) {
        return Err(ImageLoadError::UnsupportedObject(
            "object contains no executable section".into(),
        ));
    }

    // Windows unwind RVA 以整块 image base 为基准，因此代码段固定排在首个 page；
    // 其余平台沿用同一布局，避免产生第二套地址模型。
    prepared.sort_by_key(|section| {
        (
            !section.executable,
            section.name != ".text" && section.name != "__text",
        )
    });
    let page_size = page_size()?;
    let mut cursor = 0usize;
    let mut sections = Vec::with_capacity(prepared.len());
    for section in &prepared {
        let mapping_offset = cursor
            .checked_next_multiple_of(page_size)
            .ok_or(ImageLoadError::AddressOverflow)?;
        let mapping_len = align_to_page(section.loaded_len)?;
        cursor = mapping_offset
            .checked_add(mapping_len)
            .ok_or(ImageLoadError::AddressOverflow)?;
        sections.push(LoadedSection {
            index: Some(section.index),
            name: section.name.clone(),
            mapping_offset,
            data_len: section.data_len,
            mapping_len,
            loaded_len: section.loaded_len,
            executable: section.executable,
            call_stubs: HashMap::with_capacity(section.call_targets.len()),
            got_slots: HashMap::with_capacity(section.got_keys.len()),
        });
    }
    let mut mapping = ExecutableMapping::allocate(cursor)?;
    for (position, section) in prepared.into_iter().enumerate() {
        let loaded = &mut sections[position];
        let data = file.section_by_index(section.index)?.data()?;
        mapping.write(loaded.mapping_offset, data)?;
        for (index, (key, target)) in section.call_targets.into_iter().enumerate() {
            let relative_offset = section
                .data_len
                .checked_add(index.saturating_mul(call_stub_size()))
                .ok_or(ImageLoadError::AddressOverflow)?;
            let offset = loaded
                .mapping_offset
                .checked_add(relative_offset)
                .ok_or(ImageLoadError::AddressOverflow)?;
            mapping.write(offset, &call_stub_bytes(target)?)?;
            loaded.call_stubs.insert(
                key,
                u64::try_from(relative_offset).map_err(|_| ImageLoadError::AddressOverflow)?,
            );
        }
        let call_end = section
            .data_len
            .checked_add(loaded.call_stubs.len().saturating_mul(call_stub_size()))
            .ok_or(ImageLoadError::AddressOverflow)?;
        let got_base = call_end
            .checked_add(7)
            .map(|offset| offset & !7)
            .ok_or(ImageLoadError::AddressOverflow)?;
        for (index, key) in section.got_keys.into_iter().enumerate() {
            let relative_offset = got_base
                .checked_add(
                    index
                        .checked_mul(size_of::<u64>())
                        .ok_or(ImageLoadError::AddressOverflow)?,
                )
                .ok_or(ImageLoadError::AddressOverflow)?;
            loaded.got_slots.insert(
                key,
                u64::try_from(relative_offset).map_err(|_| ImageLoadError::AddressOverflow)?,
            );
        }
    }
    Ok(LoadedImage { mapping, sections })
}

fn external_call_targets(
    file: &object::File<'_>,
    section_index: SectionIndex,
    resolver: &dyn NativeSymbolResolver,
) -> Result<Vec<(RelocationKey, usize)>, ImageLoadError> {
    let section = file.section_by_index(section_index)?;
    let mut targets = Vec::new();
    for (_, relocation) in section.relocations() {
        if !matches!(
            relocation.kind(),
            RelocationKind::PltRelative | RelocationKind::Relative
        ) || !matches!(relocation.size(), 26 | 32)
        {
            continue;
        }
        let RelocationTarget::Symbol(index) = relocation.target() else {
            continue;
        };
        let symbol = file.symbol_by_index(index)?;
        if symbol.section() != SymbolSection::Undefined {
            continue;
        }
        let key = RelocationKey::Symbol(index);
        if targets.iter().any(|(existing, _)| *existing == key) {
            continue;
        }
        let name = symbol.name()?;
        let target = resolve_process_symbol(name, resolver)
            .ok_or_else(|| ImageLoadError::UnknownSymbol(name.to_owned()))?;
        targets.push((key, target));
    }
    Ok(targets)
}

fn local_got_keys(
    file: &object::File<'_>,
    section_index: SectionIndex,
) -> Result<Vec<RelocationKey>, ImageLoadError> {
    let section = file.section_by_index(section_index)?;
    let mut keys = Vec::new();
    for (_, relocation) in section.relocations() {
        let raw = aarch64_relocation(relocation.flags());
        if relocation.kind() != RelocationKind::GotRelative
            && !raw.is_some_and(Aarch64Relocation::uses_got)
        {
            continue;
        }
        let key = relocation_key(relocation.target())?;
        if !keys.contains(&key) {
            keys.push(key);
        }
    }
    Ok(keys)
}

#[cfg(target_arch = "x86_64")]
const fn call_stub_size() -> usize {
    12
}

#[cfg(target_arch = "x86_64")]
fn call_stub_bytes(target: usize) -> Result<Vec<u8>, ImageLoadError> {
    let target = u64::try_from(target).map_err(|_| ImageLoadError::AddressOverflow)?;
    let mut bytes = Vec::with_capacity(call_stub_size());
    bytes.extend_from_slice(&[0x48, 0xb8]);
    bytes.extend_from_slice(&target.to_le_bytes());
    bytes.extend_from_slice(&[0xff, 0xe0]);
    Ok(bytes)
}

#[cfg(target_arch = "aarch64")]
const fn call_stub_size() -> usize {
    16
}

#[cfg(target_arch = "aarch64")]
fn call_stub_bytes(target: usize) -> Result<Vec<u8>, ImageLoadError> {
    let target = u64::try_from(target).map_err(|_| ImageLoadError::AddressOverflow)?;
    let mut bytes = Vec::with_capacity(call_stub_size());
    bytes.extend_from_slice(&0x5800_0050_u32.to_le_bytes());
    bytes.extend_from_slice(&0xd61f_0200_u32.to_le_bytes());
    bytes.extend_from_slice(&target.to_le_bytes());
    Ok(bytes)
}

fn is_code_section(name: &str) -> bool {
    name == ".text" || name.starts_with(".text.") || name == "__text"
}

fn is_read_only_section(name: &str) -> bool {
    name == ".rodata"
        || name.starts_with(".rodata.")
        || name.starts_with(".rdata")
        || name == ".eh_frame"
        || name == ".pdata"
        || name == ".xdata"
        || name.contains("__const")
        || name.contains("__eh_frame")
}

fn is_ignored_section(name: &str) -> bool {
    name.is_empty()
        || name == ".symtab"
        || name == ".strtab"
        || name == ".shstrtab"
        || name.starts_with(".debug")
        || name.starts_with(".rela.debug")
        || name.starts_with(".rela.")
        || name.starts_with(".rel.")
        || name.starts_with(".comment")
        || name == ".note.GNU-stack"
        || name == ".llvm_addrsig"
        || name == "__compact_unwind"
        || name == "__unwind_info"
        || name.starts_with("__llvm_prf")
}

fn apply_relocations(
    file: &object::File<'_>,
    loaded: &mut LoadedImage,
    resolver: &dyn NativeSymbolResolver,
) -> Result<(), ImageLoadError> {
    let locations: HashMap<_, _> = loaded
        .sections
        .iter()
        .enumerate()
        .filter_map(|(position, section)| section.index.map(|index| (index, position)))
        .collect();
    for source in file.sections() {
        let Some(&source_position) = locations.get(&source.index()) else {
            continue;
        };
        for (offset, relocation) in source.relocations() {
            let source_section = &loaded.sections[source_position];
            if let Some(kind) = aarch64_relocation(relocation.flags()) {
                let target = if kind.uses_got() {
                    let key = relocation_key(relocation.target())?;
                    let slot_offset =
                        source_section.got_slots.get(&key).copied().ok_or_else(|| {
                            ImageLoadError::UnsupportedRelocation("missing AArch64 GOT slot".into())
                        })?;
                    let resolved = relocation_target(
                        file,
                        &locations,
                        &loaded.mapping,
                        &loaded.sections,
                        relocation.target(),
                        resolver,
                    )?;
                    let write_offset = source_section
                        .mapping_offset
                        .checked_add(
                            usize::try_from(slot_offset)
                                .map_err(|_| ImageLoadError::AddressOverflow)?,
                        )
                        .ok_or(ImageLoadError::AddressOverflow)?;
                    loaded.mapping.write_u64(
                        write_offset,
                        u64::try_from(resolved).map_err(|_| ImageLoadError::AddressOverflow)?,
                    )?;
                    source_section.address(&loaded.mapping, slot_offset)?
                } else {
                    relocation_target(
                        file,
                        &locations,
                        &loaded.mapping,
                        &loaded.sections,
                        relocation.target(),
                        resolver,
                    )?
                };
                let place = source_section.address(&loaded.mapping, offset)?;
                let write_offset = source_section
                    .mapping_offset
                    .checked_add(
                        usize::try_from(offset).map_err(|_| ImageLoadError::AddressOverflow)?,
                    )
                    .ok_or(ImageLoadError::AddressOverflow)?;
                apply_aarch64_relocation(
                    &mut loaded.mapping,
                    write_offset,
                    kind,
                    target,
                    relocation.addend(),
                    place,
                )?;
                continue;
            }
            let call_stub = match relocation.target() {
                RelocationTarget::Symbol(index) => {
                    source_section.call_stubs.get(&RelocationKey::Symbol(index))
                }
                RelocationTarget::Section(index) => source_section
                    .call_stubs
                    .get(&RelocationKey::Section(index)),
                _ => None,
            };
            let target = if let Some(offset) = call_stub {
                source_section.address(&loaded.mapping, *offset)?
            } else if relocation.kind() == RelocationKind::GotRelative {
                let key = relocation_key(relocation.target())?;
                let slot_offset = source_section.got_slots.get(&key).copied().ok_or_else(|| {
                    ImageLoadError::UnsupportedRelocation("missing local GOT slot".into())
                })?;
                let resolved = relocation_target(
                    file,
                    &locations,
                    &loaded.mapping,
                    &loaded.sections,
                    relocation.target(),
                    resolver,
                )?;
                let write_offset = source_section
                    .mapping_offset
                    .checked_add(
                        usize::try_from(slot_offset)
                            .map_err(|_| ImageLoadError::AddressOverflow)?,
                    )
                    .ok_or(ImageLoadError::AddressOverflow)?;
                loaded.mapping.write_u64(
                    write_offset,
                    u64::try_from(resolved).map_err(|_| ImageLoadError::AddressOverflow)?,
                )?;
                source_section.address(&loaded.mapping, slot_offset)?
            } else {
                relocation_target(
                    file,
                    &locations,
                    &loaded.mapping,
                    &loaded.sections,
                    relocation.target(),
                    resolver,
                )?
            };
            let place = source_section.address(&loaded.mapping, offset)?;
            let value = relocation_value(
                relocation.kind(),
                relocation.size(),
                target,
                relocation.addend(),
                place,
                loaded.mapping.address(),
            )?;
            let offset = source_section
                .mapping_offset
                .checked_add(usize::try_from(offset).map_err(|_| ImageLoadError::AddressOverflow)?)
                .ok_or(ImageLoadError::AddressOverflow)?;
            write_relocation(
                &mut loaded.mapping,
                offset,
                relocation.kind(),
                relocation.size(),
                value,
            )?;
        }
    }
    Ok(())
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
enum RelocationKey {
    Symbol(object::SymbolIndex),
    Section(SectionIndex),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Aarch64Relocation {
    GotPage21,
    GotPageOffset12,
    Page21,
    PageOffset12,
}

impl Aarch64Relocation {
    const fn uses_got(self) -> bool {
        matches!(self, Self::GotPage21 | Self::GotPageOffset12)
    }
}

fn aarch64_relocation(flags: RelocationFlags) -> Option<Aarch64Relocation> {
    match flags {
        RelocationFlags::Elf { r_type } => match r_type {
            object::elf::R_AARCH64_ADR_GOT_PAGE => Some(Aarch64Relocation::GotPage21),
            object::elf::R_AARCH64_LD64_GOT_LO12_NC => Some(Aarch64Relocation::GotPageOffset12),
            object::elf::R_AARCH64_ADR_PREL_PG_HI21 => Some(Aarch64Relocation::Page21),
            object::elf::R_AARCH64_ADD_ABS_LO12_NC => Some(Aarch64Relocation::PageOffset12),
            _ => None,
        },
        RelocationFlags::MachO { r_type, .. } => match r_type {
            object::macho::ARM64_RELOC_GOT_LOAD_PAGE21 => Some(Aarch64Relocation::GotPage21),
            object::macho::ARM64_RELOC_GOT_LOAD_PAGEOFF12 => {
                Some(Aarch64Relocation::GotPageOffset12)
            }
            object::macho::ARM64_RELOC_PAGE21 => Some(Aarch64Relocation::Page21),
            object::macho::ARM64_RELOC_PAGEOFF12 => Some(Aarch64Relocation::PageOffset12),
            _ => None,
        },
        _ => None,
    }
}

fn relocation_key(target: RelocationTarget) -> Result<RelocationKey, ImageLoadError> {
    match target {
        RelocationTarget::Symbol(index) => Ok(RelocationKey::Symbol(index)),
        RelocationTarget::Section(index) => Ok(RelocationKey::Section(index)),
        target => Err(ImageLoadError::UnsupportedRelocation(format!(
            "GOT target {target:?}"
        ))),
    }
}

fn write_relocation(
    mapping: &mut ExecutableMapping,
    offset: usize,
    kind: RelocationKind,
    size: u8,
    value: i128,
) -> Result<(), ImageLoadError> {
    let out_of_range = || {
        ImageLoadError::RelocationOutOfRange(format!("writing {kind:?} {size}-bit value {value}",))
    };
    match (kind, size) {
        (RelocationKind::Relative | RelocationKind::PltRelative, 26) => {
            let value = i64::try_from(value).map_err(|_| out_of_range())?;
            if value % 4 != 0 {
                return Err(out_of_range());
            }
            let instruction = mapping.read_u32(offset)?;
            let immediate =
                u32::try_from((value >> 2) & 0x03ff_ffff).map_err(|_| out_of_range())?;
            mapping.write_u32(offset, (instruction & 0xfc00_0000) | immediate)
        }
        (
            RelocationKind::Relative | RelocationKind::PltRelative | RelocationKind::GotRelative,
            32,
        ) => {
            let value = i32::try_from(value).map_err(|_| out_of_range())?;
            mapping.write(offset, &value.to_le_bytes())
        }
        (RelocationKind::Absolute | RelocationKind::ImageOffset, 32) => {
            mapping.write_u32(offset, u32::try_from(value).map_err(|_| out_of_range())?)
        }
        (RelocationKind::Absolute, 64) => {
            mapping.write_u64(offset, u64::try_from(value).map_err(|_| out_of_range())?)
        }
        _ => Err(ImageLoadError::UnsupportedRelocation(format!(
            "kind {kind:?} size {size}"
        ))),
    }
}

fn apply_aarch64_relocation(
    mapping: &mut ExecutableMapping,
    offset: usize,
    kind: Aarch64Relocation,
    target: usize,
    addend: i64,
    place: usize,
) -> Result<(), ImageLoadError> {
    let target = i128::try_from(target)
        .map_err(|_| ImageLoadError::AddressOverflow)?
        .checked_add(i128::from(addend))
        .ok_or(ImageLoadError::AddressOverflow)?;
    let target = usize::try_from(target).map_err(|_| {
        ImageLoadError::RelocationOutOfRange(format!(
            "{kind:?} target with addend is outside the host address space"
        ))
    })?;
    let instruction = mapping.read_u32(offset)?;
    let instruction = match kind {
        Aarch64Relocation::GotPage21 | Aarch64Relocation::Page21 => {
            encode_aarch64_page21(instruction, target, place)?
        }
        Aarch64Relocation::GotPageOffset12 => {
            encode_aarch64_page_offset12(instruction, target, true)?
        }
        Aarch64Relocation::PageOffset12 => {
            encode_aarch64_page_offset12(instruction, target, false)?
        }
    };
    mapping.write_u32(offset, instruction)
}

fn encode_aarch64_page21(
    instruction: u32,
    target: usize,
    place: usize,
) -> Result<u32, ImageLoadError> {
    if instruction & 0x9f00_0000 != 0x9000_0000 {
        return Err(ImageLoadError::UnsupportedRelocation(
            "AArch64 PAGE21 relocation does not reference ADRP".into(),
        ));
    }
    let target_page =
        i128::try_from(target & !0xfff).map_err(|_| ImageLoadError::AddressOverflow)?;
    let place_page = i128::try_from(place & !0xfff).map_err(|_| ImageLoadError::AddressOverflow)?;
    let pages = (target_page - place_page) >> 12;
    if !(-(1_i128 << 20)..(1_i128 << 20)).contains(&pages) {
        return Err(ImageLoadError::RelocationOutOfRange(format!(
            "AArch64 PAGE21 target={target:#x} place={place:#x} pages={pages}"
        )));
    }
    let immediate =
        u32::try_from(pages & 0x1f_ffff).map_err(|_| ImageLoadError::AddressOverflow)?;
    let immlo = immediate & 0x3;
    let immhi = (immediate >> 2) & 0x7_ffff;
    Ok((instruction & 0x9f00_001f) | (immlo << 29) | (immhi << 5))
}

fn encode_aarch64_page_offset12(
    instruction: u32,
    target: usize,
    scaled_load: bool,
) -> Result<u32, ImageLoadError> {
    let page_offset = target & 0xfff;
    let immediate = if scaled_load {
        if instruction & 0xffc0_0000 != 0xf940_0000 || !page_offset.is_multiple_of(8) {
            return Err(ImageLoadError::UnsupportedRelocation(
                "AArch64 GOT PAGEOFF12 relocation does not reference aligned LDR x".into(),
            ));
        }
        page_offset / 8
    } else {
        if instruction & 0x7f00_0000 != 0x1100_0000 {
            return Err(ImageLoadError::UnsupportedRelocation(
                "AArch64 PAGEOFF12 relocation does not reference ADD immediate".into(),
            ));
        }
        page_offset
    };
    let immediate = u32::try_from(immediate).map_err(|_| ImageLoadError::AddressOverflow)?;
    Ok((instruction & !(0xfff << 10)) | (immediate << 10))
}

fn relocation_target(
    file: &object::File<'_>,
    locations: &HashMap<SectionIndex, usize>,
    mapping: &ExecutableMapping,
    sections: &[LoadedSection],
    target: RelocationTarget,
    resolver: &dyn NativeSymbolResolver,
) -> Result<usize, ImageLoadError> {
    match target {
        RelocationTarget::Symbol(index) => {
            let symbol = file.symbol_by_index(index)?;
            match symbol.section() {
                SymbolSection::Section(section) => {
                    let position = locations
                        .get(&section)
                        .copied()
                        .ok_or_else(|| ImageLoadError::UnknownSymbol(symbol_name(&symbol)))?;
                    sections[position].address(mapping, symbol.address())
                }
                SymbolSection::Undefined => {
                    let name = symbol.name()?;
                    resolve_process_symbol(name, resolver)
                        .ok_or_else(|| ImageLoadError::UnknownSymbol(name.to_owned()))
                }
                _ => Err(ImageLoadError::UnknownSymbol(symbol_name(&symbol))),
            }
        }
        RelocationTarget::Section(section) => {
            let position = locations
                .get(&section)
                .copied()
                .ok_or_else(|| ImageLoadError::UnknownSymbol(format!("section {section:?}")))?;
            sections[position].address(mapping, 0)
        }
        target => Err(ImageLoadError::UnsupportedRelocation(format!(
            "target {target:?}"
        ))),
    }
}

fn relocation_value(
    kind: RelocationKind,
    size: u8,
    target: usize,
    addend: i64,
    place: usize,
    image_base: usize,
) -> Result<i128, ImageLoadError> {
    let target = i128::try_from(target).map_err(|_| ImageLoadError::AddressOverflow)?;
    let place = i128::try_from(place).map_err(|_| ImageLoadError::AddressOverflow)?;
    let image_base = i128::try_from(image_base).map_err(|_| ImageLoadError::AddressOverflow)?;
    let addend = i128::from(addend);
    let value = match kind {
        RelocationKind::Absolute => target + addend,
        RelocationKind::ImageOffset => target + addend - image_base,
        RelocationKind::Relative | RelocationKind::PltRelative | RelocationKind::GotRelative => {
            target + addend - place
        }
        other => {
            return Err(ImageLoadError::UnsupportedRelocation(format!(
                "kind {other:?}"
            )));
        }
    };
    let valid = match (kind, size) {
        (RelocationKind::Relative | RelocationKind::PltRelative, 26) => {
            value % 4 == 0 && (-(1_i128 << 27)..(1_i128 << 27)).contains(&value)
        }
        (
            RelocationKind::Relative | RelocationKind::PltRelative | RelocationKind::GotRelative,
            32,
        ) => (i128::from(i32::MIN)..=i128::from(i32::MAX)).contains(&value),
        (RelocationKind::Absolute | RelocationKind::ImageOffset, 32) => {
            (0..=i128::from(u32::MAX)).contains(&value)
        }
        (RelocationKind::Absolute, 64) => (0..=i128::from(u64::MAX)).contains(&value),
        _ => {
            return Err(ImageLoadError::UnsupportedRelocation(format!(
                "kind {kind:?} size {size}"
            )));
        }
    };
    if valid {
        Ok(value)
    } else {
        Err(ImageLoadError::RelocationOutOfRange(format!(
            "{kind:?} {size}-bit target={target:#x} place={place:#x} addend={addend} value={value}",
        )))
    }
}
#[cfg(not(target_os = "windows"))]
fn patch_platform_unwind(
    _file: &object::File<'_>,
    _loaded: &mut LoadedImage,
) -> Result<(), ImageLoadError> {
    Ok(())
}

#[cfg(target_os = "windows")]
fn patch_platform_unwind(
    _file: &object::File<'_>,
    loaded: &mut LoadedImage,
) -> Result<(), ImageLoadError> {
    let mut code = loaded.sections.iter().filter(|section| section.executable);
    let text = code.next().ok_or_else(|| {
        ImageLoadError::UnsupportedObject("Windows native object contains no text section".into())
    })?;
    if code.next().is_some() || text.mapping_offset != 0 {
        return Err(ImageLoadError::UnsupportedObject(
            "Windows native object must contain one leading text section".into(),
        ));
    }
    let xdata = loaded
        .sections
        .iter()
        .find(|section| section.name == ".xdata")
        .ok_or_else(|| {
            ImageLoadError::UnsupportedObject("Windows native object is missing .xdata".into())
        })?;
    let pdata = loaded
        .sections
        .iter()
        .find(|section| section.name == ".pdata")
        .ok_or_else(|| {
            ImageLoadError::UnsupportedObject("Windows native object is missing .pdata".into())
        })?;
    let entry_size = size_of::<PlatformRuntimeFunction>();
    if pdata.data_len == 0 || !pdata.data_len.is_multiple_of(entry_size) {
        return Err(ImageLoadError::UnsupportedObject(
            "Windows .pdata has an invalid runtime-function table length".into(),
        ));
    }
    let text_rva =
        u32::try_from(text.mapping_offset).map_err(|_| ImageLoadError::AddressOverflow)?;
    let xdata_rva =
        u32::try_from(xdata.mapping_offset).map_err(|_| ImageLoadError::AddressOverflow)?;
    let pdata_offset = pdata.mapping_offset;
    for entry in (0..pdata.data_len).step_by(entry_size) {
        let offset = pdata_offset
            .checked_add(entry)
            .ok_or(ImageLoadError::AddressOverflow)?;
        let begin = loaded
            .mapping
            .read_u32(offset)?
            .checked_add(text_rva)
            .ok_or(ImageLoadError::AddressOverflow)?;
        loaded.mapping.write_u32(offset, begin)?;

        #[cfg(target_arch = "x86_64")]
        let unwind_field = {
            let end_offset = offset
                .checked_add(size_of::<u32>())
                .ok_or(ImageLoadError::AddressOverflow)?;
            let end = loaded
                .mapping
                .read_u32(end_offset)?
                .checked_add(text_rva)
                .ok_or(ImageLoadError::AddressOverflow)?;
            loaded.mapping.write_u32(end_offset, end)?;
            end_offset
                .checked_add(size_of::<u32>())
                .ok_or(ImageLoadError::AddressOverflow)?
        };
        #[cfg(target_arch = "aarch64")]
        let unwind_field = offset
            .checked_add(size_of::<u32>())
            .ok_or(ImageLoadError::AddressOverflow)?;

        let unwind = loaded
            .mapping
            .read_u32(unwind_field)?
            .checked_add(xdata_rva)
            .ok_or(ImageLoadError::AddressOverflow)?;
        loaded.mapping.write_u32(unwind_field, unwind)?;
    }
    Ok(())
}

fn finalize_sections(loaded: &LoadedImage) -> Result<(), ImageLoadError> {
    for section in &loaded.sections {
        if section.executable {
            loaded
                .mapping
                .finalize_executable(section.mapping_offset, section.mapping_len)?;
        } else {
            loaded
                .mapping
                .finalize_read_only(section.mapping_offset, section.mapping_len)?;
        }
    }
    Ok(())
}

fn build_entries(
    file: &object::File<'_>,
    loaded: &LoadedImage,
    object: &NativeObject,
    image_id: u64,
) -> Result<Vec<NativeFunctionEntry>, ImageLoadError> {
    let locations: HashMap<_, _> = loaded
        .sections
        .iter()
        .enumerate()
        .filter_map(|(position, section)| section.index.map(|index| (index, position)))
        .collect();
    let count =
        usize::try_from(object.function_count()).map_err(|_| ImageLoadError::AddressOverflow)?;
    let mut entries = Vec::with_capacity(count);
    for index in 0..count {
        let name = format!("wjsm_function_{index}");
        let symbol = file
            .symbol_by_name(&name)
            .ok_or_else(|| ImageLoadError::UnknownSymbol(name.clone()))?;
        let SymbolSection::Section(section) = symbol.section() else {
            return Err(ImageLoadError::UnknownSymbol(name));
        };
        let position = locations
            .get(&section)
            .copied()
            .ok_or_else(|| ImageLoadError::UnknownSymbol(name.clone()))?;
        let address = loaded.sections[position].address(&loaded.mapping, symbol.address())?;
        let pointer = std::ptr::with_exposed_provenance::<u8>(address);
        // SAFETY: symbol 指向已验证、已重定位并最终设为 RX 的 slow-entry 函数；签名由 compiler
        // 对所有导出统一生成，映射由返回的 CompiledImage 持有到 entry 不再可达为止。
        let slow_entry = unsafe { std::mem::transmute::<*const u8, NativeSlowEntry>(pointer) };
        entries.push(NativeFunctionEntry {
            slow_entry,
            local_function_id: u32::try_from(index).map_err(|_| ImageLoadError::AddressOverflow)?,
            frame_bytes: object.frame_bytes()[index],
            image_id,
            osr_entry: std::sync::atomic::AtomicUsize::new(0),
        });
    }
    Ok(entries)
}

fn resolve_exported_symbol_address(
    file: &object::File<'_>,
    loaded: &LoadedImage,
    name: &str,
) -> Option<usize> {
    let symbol = file.symbol_by_name(name)?;
    let SymbolSection::Section(section) = symbol.section() else {
        return None;
    };
    let position = loaded
        .sections
        .iter()
        .enumerate()
        .find_map(|(position, loaded_section)| {
            loaded_section
                .index
                .filter(|index| *index == section)
                .map(|_| position)
        })?;
    loaded.sections[position]
        .address(&loaded.mapping, symbol.address())
        .ok()
}

fn resolve_process_symbol(name: &str, resolver: &dyn NativeSymbolResolver) -> Option<usize> {
    if let Some(symbol) = NativeHostSymbol::from_symbol_name(name) {
        return resolver.resolve(symbol);
    }
    match name {
        "wjsm_native_memory_copy" => Some((native_memory_copy as *const ()).addr()),
        "wjsm_native_memory_move" => Some((native_memory_move as *const ()).addr()),
        "wjsm_native_memory_fill" => Some((native_memory_fill as *const ()).addr()),
        "wjsm_native_memory_compare" => Some((native_memory_compare as *const ()).addr()),
        _ => None,
    }
}

unsafe extern "C" fn native_memory_copy(
    destination: *mut u8,
    source: *const u8,
    len: usize,
) -> *mut u8 {
    // SAFETY: generated code only emits this libcall after explicit range validation; C memcpy
    // contract requires valid, non-overlapping regions of `len` bytes.
    unsafe { std::ptr::copy_nonoverlapping(source, destination, len) };
    destination
}

unsafe extern "C" fn native_memory_move(
    destination: *mut u8,
    source: *const u8,
    len: usize,
) -> *mut u8 {
    // SAFETY: generated code validates both ranges; `ptr::copy` permits overlap.
    unsafe { std::ptr::copy(source, destination, len) };
    destination
}

unsafe extern "C" fn native_memory_fill(destination: *mut u8, byte: i32, len: usize) -> *mut u8 {
    // SAFETY: generated code validates the destination range for `len` bytes.
    unsafe { std::ptr::write_bytes(destination, byte.cast_unsigned() as u8, len) };
    destination
}

unsafe extern "C" fn native_memory_compare(left: *const u8, right: *const u8, len: usize) -> i32 {
    for index in 0..len {
        // SAFETY: generated code validates both ranges for `len` bytes before calling.
        let left = unsafe { *left.add(index) };
        // SAFETY: same validated range invariant as above.
        let right = unsafe { *right.add(index) };
        match left.cmp(&right) {
            std::cmp::Ordering::Less => return -1,
            std::cmp::Ordering::Greater => return 1,
            std::cmp::Ordering::Equal => {}
        }
    }
    0
}

fn symbol_name(symbol: &object::Symbol<'_, '_>) -> String {
    symbol.name().unwrap_or("<invalid symbol>").to_owned()
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
struct UnwindRegistration {
    registrations: Vec<*const c_void>,
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
impl Drop for UnwindRegistration {
    fn drop(&mut self) {
        unsafe extern "C" {
            fn __deregister_frame(address: *const c_void);
        }
        for address in self.registrations.iter().rev() {
            // SAFETY: 每个 address 均由本 image 注册一次，且 mapping 在 token 之后释放。
            unsafe { __deregister_frame(*address) };
        }
    }
}

#[cfg(target_os = "linux")]
fn register_unwind(loaded: &LoadedImage) -> Result<UnwindRegistration, ImageLoadError> {
    let section = unix_unwind_section(loaded)?;
    if section.data_len < size_of::<u32>() {
        return Err(ImageLoadError::UnsupportedObject(
            "native object contains a truncated .eh_frame".into(),
        ));
    }
    let address = std::ptr::with_exposed_provenance::<c_void>(section.address(&loaded.mapping, 0)?);
    unsafe extern "C" {
        fn __register_frame(address: *const c_void);
    }
    // SAFETY: GNU libgcc 接受以零结尾的完整 `.eh_frame`，映射在注册 token 后释放。
    unsafe { __register_frame(address) };
    Ok(UnwindRegistration {
        registrations: vec![address],
    })
}

#[cfg(target_os = "macos")]
fn register_unwind(loaded: &LoadedImage) -> Result<UnwindRegistration, ImageLoadError> {
    let section = unix_unwind_section(loaded)?;
    let mut registrations = Vec::new();
    let mut cursor = 0usize;
    let mut terminated = false;
    unsafe extern "C" {
        fn __register_frame(address: *const c_void);
    }
    while cursor < section.data_len {
        let entry = section
            .mapping_offset
            .checked_add(cursor)
            .ok_or(ImageLoadError::AddressOverflow)?;
        let len = loaded.mapping.read_u32(entry)?;
        if len == 0 {
            terminated = true;
            break;
        }
        if len == u32::MAX {
            return Err(ImageLoadError::UnsupportedObject(
                "DWARF64 .eh_frame entries are unsupported".into(),
            ));
        }
        let entry_len = usize::try_from(len)
            .map_err(|_| ImageLoadError::AddressOverflow)?
            .checked_add(size_of::<u32>())
            .ok_or(ImageLoadError::AddressOverflow)?;
        let end = cursor
            .checked_add(entry_len)
            .ok_or(ImageLoadError::AddressOverflow)?;
        if end > section.data_len {
            return Err(ImageLoadError::UnsupportedObject(
                "macOS .eh_frame entry is truncated".into(),
            ));
        }
        let cie_pointer = loaded.mapping.read_u32(
            entry
                .checked_add(size_of::<u32>())
                .ok_or(ImageLoadError::AddressOverflow)?,
        )?;
        if cie_pointer != 0 {
            let address = std::ptr::with_exposed_provenance::<c_void>(section.address(
                &loaded.mapping,
                u64::try_from(cursor).map_err(|_| ImageLoadError::AddressOverflow)?,
            )?);
            // SAFETY: Apple libunwind 接受单个 FDE；当前 entry 已验证完整且不是 CIE。
            unsafe { __register_frame(address) };
            registrations.push(address);
        }
        cursor = end;
    }
    if !terminated || registrations.is_empty() {
        return Err(ImageLoadError::UnsupportedObject(
            "macOS .eh_frame lacks a terminator or FDE".into(),
        ));
    }
    Ok(UnwindRegistration { registrations })
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn unix_unwind_section(loaded: &LoadedImage) -> Result<&LoadedSection, ImageLoadError> {
    loaded
        .sections
        .iter()
        .find(|section| section.name == ".eh_frame" || section.name.contains("__eh_frame"))
        .ok_or_else(|| {
            ImageLoadError::UnsupportedObject("native object is missing .eh_frame".into())
        })
}

#[cfg(target_os = "windows")]
struct UnwindRegistration {
    table: *const PlatformRuntimeFunction,
}

#[cfg(target_os = "windows")]
impl Drop for UnwindRegistration {
    fn drop(&mut self) {
        // SAFETY: table 已由 RtlAddFunctionTable 注册一次，mapping 在 token 之后释放。
        let _ = unsafe { RtlDeleteFunctionTable(self.table) };
    }
}

#[cfg(target_os = "windows")]
fn register_unwind(loaded: &LoadedImage) -> Result<UnwindRegistration, ImageLoadError> {
    let pdata = loaded
        .sections
        .iter()
        .find(|section| section.name == ".pdata")
        .ok_or_else(|| {
            ImageLoadError::UnsupportedObject("Windows native object is missing .pdata".into())
        })?;
    let entry_size = size_of::<PlatformRuntimeFunction>();
    if pdata.data_len == 0 || !pdata.data_len.is_multiple_of(entry_size) {
        return Err(ImageLoadError::UnsupportedObject(
            "Windows .pdata has an invalid runtime-function table length".into(),
        ));
    }
    let table_address = pdata.address(&loaded.mapping, 0)?;
    let table = std::ptr::with_exposed_provenance::<PlatformRuntimeFunction>(table_address);
    let count =
        u32::try_from(pdata.data_len / entry_size).map_err(|_| ImageLoadError::AddressOverflow)?;
    #[cfg(target_arch = "x86_64")]
    // SAFETY: pdata 是已修补、4 字节对齐且仍存活的 runtime-function 数组；RVA 以 mapping base 为准。
    let registered = unsafe {
        RtlAddFunctionTable(
            table,
            count,
            u64::try_from(loaded.mapping.address()).map_err(|_| ImageLoadError::AddressOverflow)?,
        )
    };
    #[cfg(target_arch = "aarch64")]
    // SAFETY: 同上；ARM64 API 的 image base 参数类型为 usize。
    let registered = unsafe { RtlAddFunctionTable(table, count, loaded.mapping.address()) };
    if !registered {
        return Err(ImageLoadError::Platform(
            "RtlAddFunctionTable rejected native unwind metadata".into(),
        ));
    }
    Ok(UnwindRegistration { table })
}

#[derive(Debug, Error)]
pub enum ImageLoadError {
    #[error("invalid native object: {0}")]
    Parse(#[from] object::Error),
    #[error("unsupported native object: {0}")]
    UnsupportedObject(String),
    #[error("native object contains forbidden writable/TLS section {0}")]
    ForbiddenSection(String),
    #[error("native object contains unknown symbol {0}")]
    UnknownSymbol(String),
    #[error("unsupported native relocation: {0}")]
    UnsupportedRelocation(String),
    #[error("native relocation is out of range: {0}")]
    RelocationOutOfRange(String),
    #[error("native image address arithmetic overflow")]
    AddressOverflow,
    #[error("native image declares an invalid IC slot count")]
    InvalidIcSlotCount,
    #[error("native image declares an invalid feedback slot count")]
    InvalidFeedbackSlotCount,
    #[error("native image section access is out of bounds")]
    SectionOutOfBounds,
    #[error("native executable memory operation failed: {0}")]
    Platform(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    const ADRP_X0: u32 = 0x9000_0000;
    const ADD_X0_X0: u32 = 0x9100_0000;
    const LDR_X0_X0: u32 = 0xf940_0000;

    #[test]
    fn aarch64_object_flags_route_got_and_page_pairs() {
        assert_eq!(
            aarch64_relocation(RelocationFlags::Elf {
                r_type: object::elf::R_AARCH64_ADR_GOT_PAGE,
            }),
            Some(Aarch64Relocation::GotPage21)
        );
        assert_eq!(
            aarch64_relocation(RelocationFlags::Elf {
                r_type: object::elf::R_AARCH64_LD64_GOT_LO12_NC,
            }),
            Some(Aarch64Relocation::GotPageOffset12)
        );
        assert_eq!(
            aarch64_relocation(RelocationFlags::MachO {
                r_type: object::macho::ARM64_RELOC_PAGE21,
                r_pcrel: true,
                r_length: 2,
            }),
            Some(Aarch64Relocation::Page21)
        );
        assert_eq!(
            aarch64_relocation(RelocationFlags::MachO {
                r_type: object::macho::ARM64_RELOC_PAGEOFF12,
                r_pcrel: false,
                r_length: 2,
            }),
            Some(Aarch64Relocation::PageOffset12)
        );
    }

    #[test]
    fn aarch64_page21_encodes_signed_boundaries() {
        let place = 1usize << 32;
        let minimum = encode_aarch64_page21(ADRP_X0, 0, place)
            .expect("negative 21-bit page boundary should encode");
        assert_eq!(minimum & 0x60ff_ffe0, 0x0080_0000);

        let maximum_target = place
            .checked_add((1usize << 32) - 0x1000)
            .expect("test address should fit");
        let maximum = encode_aarch64_page21(ADRP_X0, maximum_target, place)
            .expect("positive 21-bit page boundary should encode");
        assert_eq!(maximum & 0x60ff_ffe0, 0x607f_ffe0);
    }

    #[test]
    fn aarch64_page21_rejects_range_and_opcode_errors() {
        let place = 1usize << 32;
        let out_of_range = place
            .checked_add(1usize << 32)
            .expect("test address should fit");
        assert!(matches!(
            encode_aarch64_page21(ADRP_X0, out_of_range, place),
            Err(ImageLoadError::RelocationOutOfRange(_))
        ));
        assert!(matches!(
            encode_aarch64_page21(ADD_X0_X0, place, place),
            Err(ImageLoadError::UnsupportedRelocation(_))
        ));
    }

    #[test]
    fn aarch64_page_offset12_distinguishes_add_and_scaled_load() {
        let add = encode_aarch64_page_offset12(ADD_X0_X0, 0x1fff, false)
            .expect("ADD low 12 bits should encode");
        assert_eq!((add >> 10) & 0xfff, 0xfff);

        let load = encode_aarch64_page_offset12(LDR_X0_X0, 0x1ff8, true)
            .expect("aligned LDR low 12 bits should encode");
        assert_eq!((load >> 10) & 0xfff, 0x1ff);
        assert!(matches!(
            encode_aarch64_page_offset12(LDR_X0_X0, 0x1fff, true),
            Err(ImageLoadError::UnsupportedRelocation(_))
        ));
    }

    #[test]
    fn aarch64_branch26_checks_alignment_and_signed_range() {
        assert_eq!(
            relocation_value(RelocationKind::Relative, 26, 0, 0, 1 << 27, 0)
                .expect("negative branch boundary should fit"),
            -(1_i128 << 27)
        );
        assert_eq!(
            relocation_value(RelocationKind::PltRelative, 26, (1 << 27) - 4, 0, 0, 0,)
                .expect("positive branch boundary should fit"),
            (1_i128 << 27) - 4
        );
        assert!(matches!(
            relocation_value(RelocationKind::Relative, 26, 1 << 27, 0, 0, 0),
            Err(ImageLoadError::RelocationOutOfRange(_))
        ));
        assert!(matches!(
            relocation_value(RelocationKind::Relative, 26, 2, 0, 0, 0),
            Err(ImageLoadError::RelocationOutOfRange(_))
        ));
    }

    #[test]
    fn x64_gotpcrel_and_coff_refptr_bounds_are_checked() {
        assert_eq!(
            relocation_value(RelocationKind::GotRelative, 32, 0, 0, 1 << 31, 0)
                .expect("negative GOTPCRel boundary should fit"),
            i128::from(i32::MIN)
        );
        assert_eq!(
            relocation_value(RelocationKind::Relative, 32, i32::MAX as usize, 0, 0, 0)
                .expect("positive COFF relative boundary should fit"),
            i128::from(i32::MAX)
        );
        assert!(matches!(
            relocation_value(
                RelocationKind::GotRelative,
                32,
                (i32::MAX as usize) + 1,
                0,
                0,
                0,
            ),
            Err(ImageLoadError::RelocationOutOfRange(_))
        ));
        assert_eq!(
            relocation_value(RelocationKind::ImageOffset, 32, 0x1fff, 0, 0, 0x1000)
                .expect("COFF image-relative RVA should fit"),
            0xfff
        );
    }
}
