//! 后端无关的国际化数据入口。
//!
//! ICU4X compiled_data、UTS #46（`idna`）与 WHATWG Encoding（`encoding_rs`）
//! 只允许经本 crate 进入生产二进制。禁止 Cranelift / native ABI 类型。
//! 数据由 rustc 链进 `wjsm` / `wjsm-exec` stub，不进 `.wjsm` 或 startup snapshot。
//!
//! debug 的 `wjsm` 只链 `normalize`（生产 `String.prototype.normalize` 需要）。
//! locale / format / text 在 test 与发行构建中编译，避免 debug fixture 被 3s 门禁打死。

pub mod manifest;
pub mod normalize;

#[cfg(any(test, not(debug_assertions)))]
pub mod coverage;
#[cfg(any(test, not(debug_assertions)))]
pub mod format;
#[cfg(any(test, not(debug_assertions)))]
pub mod locale;
#[cfg(any(test, not(debug_assertions)))]
pub mod text;

#[cfg(any(test, not(debug_assertions)))]
pub use encoding_rs;
#[cfg(any(test, not(debug_assertions)))]
pub use icu;
#[cfg(any(test, not(debug_assertions)))]
pub use idna;

#[cfg(any(test, not(debug_assertions)))]
pub use coverage::{CoverageError, LocaleCoverage, probe_locale};
pub use manifest::{DATA_MANIFEST, DataManifest, canonical_json, manifest_sha256};
pub use normalize::{NormalizationForm, normalize};

/// smoke matrix 使用的 locale；发行清单与测试共用。
pub const SMOKE_LOCALES: &[&str] = &[
    "en-US", "zh-CN", "de-DE", "es-ES", "ar", "th", "tr", "ja-JP",
];

/// 强制把 Phase 2/4 所需 compiled_data 留在 rustc 链接的 **发行** stub 里。
///
/// ICU4X 的 compiled_data 可被 DCE 删掉。发行构建用 `#[used]` constructor 指针
/// 留住各类数据入口，避免 Intl JS API 落地前 locale 数据从 `wjsm-exec` 消失。
/// debug `wjsm` 不编译 locale/format/text，只保留 normalize。
pub fn keep_compiled_data() {
    #[cfg(not(debug_assertions))]
    std::hint::black_box(KEEP_COMPILED_DATA_CTORS);
}

#[cfg(not(debug_assertions))]
#[used]
static KEEP_COMPILED_DATA_CTORS: &[fn()] = &[
    coverage::keep_compiled_data,
    locale::keep_locale_data,
    format::keep_format_data,
    text::keep_text_data,
];
