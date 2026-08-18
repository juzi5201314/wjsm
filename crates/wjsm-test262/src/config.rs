use std::collections::HashMap;

use crate::read::{Harness, Test, TestFlag};

/// harness include 自身声明的 features（如 temporalHelpers.js 需要 Temporal）。
pub fn include_features_from_harness(harness: &Harness) -> HashMap<String, Vec<String>> {
    let mut map = HashMap::new();
    for (name, file) in &harness.includes {
        if let Ok(meta) = crate::read::read_metadata(&file.content)
            && !meta.features.is_empty()
        {
            map.insert(name.clone(), meta.features);
        }
    }
    map
}

/// wjsm 当前已实现的 Test262 features 列表。
///
/// 每次实现新特性时，将对应的 feature 名称添加到此列表中。
pub const SUPPORTED_FEATURES: &[&str] = &[
    // 变量声明
    "let",
    "const",
    "var",
    // 控制流
    "if",
    "while",
    "do-while",
    "for",
    "for-in",
    "for-of",
    "switch",
    "break",
    "continue",
    "return",
    "try",
    "throw",
    "labeled",
    // 表达式
    "binary",
    "unary",
    "conditional",
    "update",
    "comma",
    "void",
    "typeof",
    "in",
    "instanceof",
    "delete",
    // 函数与类
    "arrow-function",
    "class",
    "class-fields-public",
    "class-fields-private",
    "class-static-fields-public",
    "class-static-fields-private",
    "class-methods-private",
    "class-static-methods-private",
    "class-static-block",
    "new",
    "super",
    "default-parameters",
    "rest-parameters",
    // 对象
    "object-literals",
    "prototype",
    "computed-property-names",
    "destructuring-assignment",
    "destructuring-binding",
    // 字面量
    "numeric-literals",
    "string-literals",
    "boolean-literals",
    "null-literal",
    "template-literal",
    // Promise / async
    "Promise",
    "async-functions",
    "async-iteration",
    // 运算符
    "arithmetic",
    "comparison",
    "equality",
    "logical-assignment",
    "logical-assignment-operators",
    "exponentiation",
    // 其他
    "debugger",
    "empty-statement",
    "globalThis",
    "Symbol",
    "Symbol.iterator",
    "Symbol.species",
    "Symbol.isConcatSpreadable",
    "Symbol.toPrimitive",
    "Symbol.toStringTag",
    "generators",
    "spread-element",
    "Map",
    "Set",
    "WeakMap",
    "WeakSet",
    "Proxy",
    "Reflect",
    "Reflect.construct",
    "Array.prototype.includes",
    "Array.prototype.flat",
    "Array.prototype.flatMap",
    "Array.prototype.at",
    "Array.prototype.findLast",
    "String.prototype.repeat",
    "String.prototype.startsWith",
    "String.prototype.endsWith",
    "String.prototype.includes",
    "String.prototype.padStart",
    "String.prototype.padEnd",
    "String.prototype.at",
    "String.prototype.trimStart",
    "String.prototype.trimEnd",
    "Object.values",
    "Object.entries",
    "Object.keys",
    "Object.assign",
    "Object.is",
    "Object.fromEntries",
    "Object.getOwnPropertyDescriptors",
    "Object.hasOwn",
    "cross-realm",
    "TypedArray",
    "BigInt",
    "ArrayBuffer",
    "DataView",
    "JSON",
    "eval",
    // Weak references
    "WeakRef",
    "FinalizationRegistry",
    // ── SharedArrayBuffer + Atomics ──
    "SharedArrayBuffer",
    "Atomics",
    "Atomics.waitAsync",
    // ── Array grouping ──
    "array-grouping",
    // ── ECMA-402（不要加光秃的 "Intl"：会前缀误选 Temporal + Intl.Era-monthcode）──
    "Intl.Locale",
    "Intl.Locale-info",
    "Intl.ListFormat",
    "Intl.RelativeTimeFormat",
    "Intl.DisplayNames",
    "Intl.DisplayNames-v2",
    "Intl.Segmenter",
    "Intl.DurationFormat",
    "Intl.NumberFormat-unified",
    "Intl.NumberFormat-v3",
    "Intl.DateTimeFormat-datetimestyle",
    "Intl.DateTimeFormat-dayPeriod",
    "Intl.DateTimeFormat-formatRange",
    "Intl.DateTimeFormat-fractionalSecondDigits",
    "Intl.DateTimeFormat-extend-timezonename",
    "Intl-enumeration",
    "Intl.Era-monthcode",
];

/// 需要忽略的 flags（wjsm 当前不支持或不适用的测试模式）。
///
/// - `CanBlockIsFalse`：wjsm 的 Agent [[CanBlock]] 为 true，不满足此 flag 的前提条件。
///
/// 注意：`TestFlag::Module` 已从忽略列表中移除——模块模式测试现在会被运行。
pub const IGNORED_FLAGS: &[TestFlag] = &[TestFlag::CanBlockIsFalse];

/// 检查是否应该运行某个测试。
///
/// - 如果 `--all` 被指定，返回 true
/// - 如果测试包含任何 IGNORED_FLAGS，返回 false
/// - 如果测试没有任何 feature 标记，返回 true（基础语法测试）
/// - 测试列出的 **全部** feature 都必须在 allowlist（分隔符感知前缀）
/// - 否则返回 false
pub fn should_run_test(
    test: &Test,
    run_all: bool,
    include_features: &HashMap<String, Vec<String>>,
) -> bool {
    if run_all {
        return true;
    }

    for flag in IGNORED_FLAGS {
        if test.metadata.flags.contains(flag) {
            return false;
        }
    }

    let mut features = test.metadata.features.clone();
    for include in &test.metadata.includes {
        if let Some(extra) = include_features.get(include) {
            features.extend(extra.iter().cloned());
        }
    }
    if features.is_empty() {
        return true;
    }

    features.iter().all(|feature| feature_supported(feature))
}

/// 精确匹配，或 `supported-` / `supported.` 前缀。避免 `"in"` 误配 `intl-*`。
fn feature_supported(feature: &str) -> bool {
    SUPPORTED_FEATURES.iter().any(|&supported| {
        feature == supported
            || feature.starts_with(&format!("{supported}-"))
            || feature.starts_with(&format!("{supported}."))
    })
}

#[cfg(test)]
mod tests {
    use super::feature_supported;

    #[test]
    fn in_does_not_match_intl_prefix() {
        assert!(feature_supported("in"));
        assert!(!feature_supported("intl-normative-optional"));
        assert!(!feature_supported("Temporal"));
        assert!(feature_supported("Intl.Locale"));
        assert!(feature_supported("Intl.Locale-info"));
    }
}
