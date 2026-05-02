use crate::read::{Test, TestFlag};

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
    "new",
    "super",
    // 对象
    "object-literals",
    "prototype",
    // 字面量
    "numeric-literals",
    "string-literals",
    "boolean-literals",
    "null-literal",
    // 运算符
    "arithmetic",
    "comparison",
    "equality",
    "logical-assignment",
    "exponentiation",
    // 其他
    "debugger",
    "empty-statement",
    "globalThis",
];

/// 需要忽略的 flags（当前 wjsm 不支持）。
pub const IGNORED_FLAGS: &[TestFlag] = &[
    TestFlag::Async,
    TestFlag::Module,
];

/// 检查是否应该运行某个测试。
///
/// - 如果 `--all` 被指定，返回 true
/// - 如果测试包含任何 IGNORED_FLAGS，返回 false
/// - 如果测试的 features 中有任何一个是 SUPPORTED_FEATURES 中的，返回 true
/// - 否则返回 false
pub fn should_run_test(test: &Test, run_all: bool) -> bool {
    if run_all {
        return true;
    }

    // 检查是否有被忽略的 flag
    for flag in IGNORED_FLAGS {
        if test.metadata.flags.contains(flag) {
            return false;
        }
    }

    // 检查是否有支持的 feature
    test.metadata.features.iter().any(|feature| {
        SUPPORTED_FEATURES.iter().any(|&supported| {
            feature == supported || feature.starts_with(&format!("{}", supported))
        })
    })
}
