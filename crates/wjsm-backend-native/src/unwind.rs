//! 跨平台 unwind object 产出。
//!
//! 三种平台策略：
//! - Linux（ELF）：`ObjectBuilder::unwind_info(true)` 由 cranelift-object 内置
//!   生成 `.eh_frame`；本模块补一个标准 length=0 终止项，保证 libgcc 遍历
//!   完整 frame table 时安全停止。
//! - macOS（Mach-O）：cranelift-object 的 Mach-O `.eh_frame` 尚未实现（pcrel
//!   SUBTRACTOR 缺失），因此用本地 gimli writer 生成 `__TEXT,__eh_frame`：
//!   CIE `fde_address_encoding` 取 `DW_EH_PE_absptr`，每个 FDE 的
//!   initial_location 以 `Address::Symbol` + Absolute/Generic/64 relocation
//!   指向函数符号；loader 应用 relocation 后即为函数绝对地址，逐 FDE 注册。
//! - Windows（COFF）：本地生成 `.xdata`（UNWIND_INFO 字节）与 `.pdata`
//!   （RUNTIME_FUNCTION 数组）。begin/end 为 `.text` section-relative 的函数
//!   范围，unwind_address 为 `.xdata` section-relative 偏移；均不预加 loaded
//!   RVA，由 loader 发布时分别叠加 text/xdata 的加载偏移后，以 mapping base
//!   调 RtlAddFunctionTable。

use cranelift_codegen::isa::unwind::UnwindInfo;
use cranelift_module::FuncId;
use cranelift_object::ObjectProduct;
use gimli::write::{
    Address, EhFrame, EndianVec, FrameTable, RelocateWriter, Relocation, RelocationTarget, Writer,
};
use object::write::{
    Relocation as ObjectRelocation, SectionKind, StandardSection, StandardSegment, SymbolId,
};
use object::{RelocationEncoding, RelocationFlags, RelocationKind};
use target_lexicon::{Architecture, OperatingSystem, Triple};

use crate::NativeCompileError;

/// 目标平台采用的 unwind object 策略。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum UnwindPolicy {
    /// Linux ELF：cranelift-object 内置 `.eh_frame`。
    SystemVObject,
    /// macOS Mach-O：本地 gimli writer，`DW_EH_PE_absptr` + Absolute/Generic/64。
    MachOAbsolute,
    /// Windows COFF：本地 `.xdata` + `.pdata`（section-relative，无 relocation）。
    WindowsPdata,
}

impl UnwindPolicy {
    /// 从目标 triple 推导 unwind object 策略；非受支持组合视为错误。
    pub(crate) fn for_triple(triple: &Triple) -> Result<Self, NativeCompileError> {
        match (&triple.architecture, &triple.operating_system) {
            (Architecture::X86_64 | Architecture::Aarch64(_), OperatingSystem::Linux) => {
                Ok(Self::SystemVObject)
            }
            (
                Architecture::X86_64 | Architecture::Aarch64(_),
                OperatingSystem::MacOSX(_) | OperatingSystem::Darwin(_),
            ) => Ok(Self::MachOAbsolute),
            (Architecture::X86_64 | Architecture::Aarch64(_), OperatingSystem::Windows) => {
                Ok(Self::WindowsPdata)
            }
            _ => Err(NativeCompileError::UnsupportedTargetCapability(format!(
                "unwind object emission is not defined for target {triple}"
            ))),
        }
    }

    /// settings_key 中 `unwind-object=` 使用的稳定名称。
    pub(crate) fn settings_name(self) -> &'static str {
        match self {
            Self::SystemVObject => "systemv",
            Self::MachOAbsolute => "macho-absolute",
            Self::WindowsPdata => "windows-pdata",
        }
    }
}

/// 每个已编译函数收集的 unwind 记录。
pub(crate) struct UnwindRecord {
    pub(crate) function: FuncId,
    pub(crate) code_len: u64,
    pub(crate) info: UnwindInfo,
}

/// 校验单个函数的 unwind info 与目标 OS/arch 期望的 variant 一致，并返回编号。
///
/// 任何受支持目标上缺 unwind info 或 variant 不符都会如实报错，绝不静默省略。
pub(crate) fn validate_unwind_info(
    triple: &Triple,
    info: &UnwindInfo,
    function: wjsm_ir::FunctionId,
) -> Result<(), NativeCompileError> {
    let expected = match (&triple.architecture, &triple.operating_system) {
        (Architecture::X86_64, OperatingSystem::Windows) => "windows-x64",
        (Architecture::Aarch64(_), OperatingSystem::Windows) => "windows-arm64",
        (Architecture::X86_64 | Architecture::Aarch64(_), _) => "systemv",
        _ => {
            return Err(NativeCompileError::UnsupportedTargetCapability(format!(
                "unwind validation is not defined for target {triple}"
            )));
        }
    };
    let actual = match info {
        UnwindInfo::WindowsX64(_) => "windows-x64",
        UnwindInfo::WindowsArm64(_) => "windows-arm64",
        UnwindInfo::SystemV(_) => "systemv",
        _ => "unknown",
    };
    if expected != actual {
        return Err(NativeCompileError::UnwindVariantMismatch {
            function,
            expected: expected.to_string(),
            actual: actual.to_string(),
        });
    }
    Ok(())
}

/// 在 `module.finish()` 之后把收集到的 unwind 记录写入 object。
pub(crate) fn write_object_unwind(
    product: &mut ObjectProduct,
    policy: UnwindPolicy,
    records: Vec<UnwindRecord>,
    systemv_cie: Option<gimli::write::CommonInformationEntry>,
    endian: gimli::RunTimeEndian,
) -> Result<(), NativeCompileError> {
    if records.is_empty() {
        return Ok(());
    }
    match policy {
        UnwindPolicy::SystemVObject => {
            // cranelift-object 已在 finish 时写出 `.eh_frame`；这里补标准终止项
            // （length=0），保证 libgcc 遍历整段时能安全停止。
            let eh_frame = product.object.section_id(StandardSection::EhFrame);
            product
                .object
                .append_section_data(eh_frame, &[0, 0, 0, 0], 1);
            Ok(())
        }
        UnwindPolicy::MachOAbsolute => {
            let cie = systemv_cie.ok_or_else(|| {
                NativeCompileError::CompilerInvariant(
                    "missing System V CIE for macho-absolute .eh_frame".into(),
                )
            })?;
            write_macho_eh_frame(product, records, cie, endian)
        }
        UnwindPolicy::WindowsPdata => write_windows_pdata(product, records),
    }
}

/// 本地生成 Mach-O `__TEXT,__eh_frame`。
///
/// CIE 使用 `DW_EH_PE_absptr`（即无 'R' augmentation 时的默认编码），每个 FDE
/// 的 initial_location 以 `Address::Symbol` 记录并转成 Absolute/Generic/64
/// relocation，避免 Mach-O pcrel SUBTRACTOR。gimli 的 `RelocateWriter` 对
/// `Address::Symbol` 写 0 占位并记录 relocation，loader 应用后即函数地址。
fn write_macho_eh_frame(
    product: &mut ObjectProduct,
    records: Vec<UnwindRecord>,
    mut cie: gimli::write::CommonInformationEntry,
    endian: gimli::RunTimeEndian,
) -> Result<(), NativeCompileError> {
    cie.fde_address_encoding = gimli::constants::DW_EH_PE_absptr;
    let mut frame_table = FrameTable::default();
    let cie_id = frame_table.add_cie(cie);
    let mut writer = MachOEhFrameWriter {
        writer: EndianVec::new(endian),
        relocations: Vec::new(),
        symbols: Vec::new(),
    };
    for record in &records {
        let UnwindInfo::SystemV(sysv) = &record.info else {
            return Err(NativeCompileError::CompilerInvariant(
                "macho-absolute requires SystemV unwind info".into(),
            ));
        };
        let symbol_index = writer.symbols.len();
        writer
            .symbols
            .push(product.function_symbol(record.function));
        let address = Address::Symbol {
            symbol: symbol_index,
            addend: 0,
        };
        frame_table.add_fde(cie_id, sysv.to_fde(address));
    }
    let mut eh_frame = EhFrame(writer);
    frame_table
        .write_eh_frame(&mut eh_frame)
        .map_err(|error| NativeCompileError::Object(error.to_string()))?;
    let MachOEhFrameWriter {
        mut writer,
        relocations,
        symbols,
    } = eh_frame.0;
    // libgcc/libunwind 期望表尾有一个 length=0 的终止项。
    writer
        .write_u32(0)
        .map_err(|error| NativeCompileError::Object(error.to_string()))?;
    let bytes = writer.into_vec();
    let section_id = product.object.section_id(StandardSection::EhFrame);
    product.object.append_section_data(section_id, &bytes, 8);
    for relocation in relocations {
        let symbol = match relocation.target {
            RelocationTarget::Symbol(index) => symbols[index],
            RelocationTarget::Section(_) => {
                return Err(NativeCompileError::CompilerInvariant(
                    "unexpected section-relative relocation in Mach-O .eh_frame".into(),
                ));
            }
        };
        if relocation.eh_pe != Some(gimli::constants::DW_EH_PE_absptr) {
            return Err(NativeCompileError::CompilerInvariant(
                "macho-absolute received a non-absptr eh_frame relocation".into(),
            ));
        }
        let flags = absolute_flags(relocation.size)?;
        let offset = u64::try_from(relocation.offset).map_err(|_| {
            NativeCompileError::CompilerInvariant("eh_frame relocation offset overflow".into())
        })?;
        product
            .object
            .add_relocation(
                section_id,
                ObjectRelocation {
                    offset,
                    symbol,
                    addend: relocation.addend,
                    flags,
                },
            )
            .map_err(|error| NativeCompileError::Object(error.to_string()))?;
    }
    Ok(())
}

/// 生成 Windows `.xdata` + `.pdata`。
///
/// `.xdata` 依次存放每个函数的 UNWIND_INFO；`.pdata` 存放 RUNTIME_FUNCTION
/// 数组（x64 每项 12 字节 begin/end/unwind_address，arm64 每项 8 字节
/// begin/unwind_address）。begin/end 是 `.text` section-relative 的函数范围，
/// unwind_address 是 `.xdata` section-relative 偏移；均不加 loaded RVA，
/// 由 loader 发布时分别叠加 text/xdata 的加载偏移后调 RtlAddFunctionTable。
fn write_windows_pdata(
    product: &mut ObjectProduct,
    records: Vec<UnwindRecord>,
) -> Result<(), NativeCompileError> {
    let mut xdata = Vec::new();
    let mut pdata = Vec::new();
    for record in &records {
        let symbol = product.function_symbol(record.function);
        let begin = product.object.symbol(symbol).value;
        let end = begin.checked_add(record.code_len).ok_or_else(|| {
            NativeCompileError::CompilerInvariant("function end offset overflow".into())
        })?;
        match &record.info {
            UnwindInfo::WindowsX64(info) => {
                let mut bytes = vec![0; info.emit_size()];
                info.emit(&mut bytes);
                pad_to_4(&mut xdata);
                let unwind_address = xdata.len();
                xdata.extend_from_slice(&bytes);
                pdata.extend_from_slice(&u32_value(begin, "pdata begin")?.to_le_bytes());
                pdata.extend_from_slice(&u32_value(end, "pdata end")?.to_le_bytes());
                pdata.extend_from_slice(
                    &u32_value(
                        u64::try_from(unwind_address).map_err(|_| {
                            NativeCompileError::CompilerInvariant(
                                "pdata unwind address exceeds u64".into(),
                            )
                        })?,
                        "pdata unwind address",
                    )?
                    .to_le_bytes(),
                );
            }
            UnwindInfo::WindowsArm64(info) => {
                let code_words = info.code_words();
                let mut unwind_codes = vec![0; usize::from(code_words) * 4];
                info.emit(&mut unwind_codes);
                pad_to_4(&mut xdata);
                // 布局与 Wasmtime `UnwindInfoBuilder::push` 严格一致：
                // 首字 0-17 函数长度（4 字节单位）、18-19 版本、20 X、
                // 21 E、22-26 epilogue 数、27-31 code words 数。
                let requires_extended_counts = code_words >= (1 << 5);
                if !record.code_len.is_multiple_of(4) {
                    return Err(NativeCompileError::CompilerInvariant(
                        "arm64 function length is not instruction-aligned".into(),
                    ));
                }

                let encoded_function_len = record.code_len / 4;
                if encoded_function_len >= (1 << 18) {
                    return Err(NativeCompileError::CompilerInvariant(
                        "arm64 function too large for unwind header".into(),
                    ));
                }
                let mut word1 = u32::try_from(encoded_function_len).map_err(|_| {
                    NativeCompileError::CompilerInvariant("arm64 function length overflow".into())
                })?;
                if !requires_extended_counts {
                    word1 |= u32::from(code_words) << 27;
                }
                let unwind_address = xdata.len();
                xdata.extend_from_slice(&word1.to_le_bytes());
                if requires_extended_counts {
                    // 扩展计数字：0-15 epilogue 数、16-23 code words 数。
                    let extended = u32::from(code_words) << 16;
                    xdata.extend_from_slice(&extended.to_le_bytes());
                }
                xdata.extend_from_slice(&unwind_codes);
                pdata.extend_from_slice(&u32_value(begin, "pdata begin")?.to_le_bytes());
                pdata.extend_from_slice(
                    &u32_value(
                        u64::try_from(unwind_address).map_err(|_| {
                            NativeCompileError::CompilerInvariant(
                                "pdata unwind address exceeds u64".into(),
                            )
                        })?,
                        "pdata unwind address",
                    )?
                    .to_le_bytes(),
                );
            }
            UnwindInfo::SystemV(_) => {
                return Err(NativeCompileError::CompilerInvariant(
                    "windows-pdata requires Windows unwind info".into(),
                ));
            }
            _ => {
                return Err(NativeCompileError::CompilerInvariant(
                    "windows-pdata received an unknown unwind variant".into(),
                ));
            }
        }
    }
    let segment = product.object.segment_name(StandardSegment::Data).to_vec();
    let xdata_id = product.object.add_section(
        segment.clone(),
        b".xdata".to_vec(),
        SectionKind::ReadOnlyData,
    );
    let pdata_id =
        product
            .object
            .add_section(segment, b".pdata".to_vec(), SectionKind::ReadOnlyData);
    product.object.append_section_data(xdata_id, &xdata, 4);
    product.object.append_section_data(pdata_id, &pdata, 4);
    Ok(())
}

/// 收集 gimli FDE 写入过程中的符号 relocation，写出时转成 object relocation。
struct MachOEhFrameWriter {
    writer: EndianVec<gimli::RunTimeEndian>,
    relocations: Vec<Relocation>,
    symbols: Vec<SymbolId>,
}

impl RelocateWriter for MachOEhFrameWriter {
    type Writer = EndianVec<gimli::RunTimeEndian>;

    fn writer(&self) -> &Self::Writer {
        &self.writer
    }

    fn writer_mut(&mut self) -> &mut Self::Writer {
        &mut self.writer
    }

    fn relocate(&mut self, relocation: Relocation) {
        self.relocations.push(relocation);
    }
}

fn absolute_flags(size: u8) -> Result<RelocationFlags, NativeCompileError> {
    let size_bits = match size {
        4 => 32,
        8 => 64,
        size => {
            return Err(NativeCompileError::CompilerInvariant(format!(
                "unexpected eh_frame pointer width {size}"
            )));
        }
    };
    Ok(RelocationFlags::Generic {
        kind: RelocationKind::Absolute,
        encoding: RelocationEncoding::Generic,
        size: size_bits,
    })
}

fn pad_to_4(bytes: &mut Vec<u8>) {
    while !bytes.len().is_multiple_of(4) {
        bytes.push(0);
    }
}

fn u32_value(value: u64, what: &str) -> Result<u32, NativeCompileError> {
    u32::try_from(value)
        .map_err(|_| NativeCompileError::CompilerInvariant(format!("{what} exceeds u32: {value}")))
}
