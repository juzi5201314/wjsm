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
    "ArrayBuffer",
    "DataView",
    "JSON",
];

/// 需要忽略的 flags（当前 wjsm 不支持）。
pub const IGNORED_FLAGS: &[TestFlag] = &[TestFlag::Module];

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
