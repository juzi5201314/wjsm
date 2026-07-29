# Program、Module 与 Function

这一章说明 IR 的顶层容器结构，以及 `Function` 上那些由语义层填写、后端消费的元数据字段。

## Module 就是 Program

`crates/wjsm-ir/src/lib.rs` 里 `pub type Program = Module;`，两个名字指同一结构。历史上 `Program` 用于「整个编译单元」，`Module` 用于「单个 JS 模块」，多模块 bundling 后二者合一：一次编译产出一个 `Module`，里面装着所有 JS 模块 lower 出的函数。

`Module` 的字段：

| 字段 | 内容 |
| --- | --- |
| `constants: Vec<Constant>` | 常量池，`ConstantId` 是下标 |
| `functions: Vec<Function>` | 函数表，`FunctionId` 是下标 |
| `script_mode: bool` | 是否按 Script 而非 Module 解析 |
| `source_file: Option<String>` | 源文件路径，供运行时错误堆栈映射 |

字段全部私有，只能经 `add_constant`、`push_function`、`constants()`、`functions()`、`function_mut()` 访问。这让「常量池只追加，`ConstantId` 一旦发出就不再变动」成为类型层面的保证，而不是靠约定。

## FunctionId 的偏移

多模块 bundling 需要把多个模块的 IR 拼进同一个 `Module`，`ModuleId` 会冲突。`offset_module_ids(offset)` 批量平移，溢出时返回 `ModuleIdOffsetError` 而不是 wrapping。这是 [Program Bundling](../modules/program-bundling.md) 依赖的原语。

## Function 的元数据

`Function` 除了 `name`、`params`、`entry`、`blocks`，还带一批供后端决策的字段：

| 字段 | 谁填 | 后端怎么用 |
| --- | --- | --- |
| `has_eval` | 语义层扫描 direct eval | 降低局部变量优化强度 |
| `captured_names` | 语义层逃逸分析 | 决定 env 对象的属性名 |
| `known_callee_vars` | 语义层（仅单次赋值的函数声明变量） | callee no-GC 分析，key 是 scope-qualified IR 名如 `$0.foo` |
| `home_object` | 语义层 | 实现 `super` 属性访问 |
| `needs_prototype` | 语义层 | 普通函数为 true，箭头/方法/类构造器为 false；决定是否创建 `prototype` 对象 |
| `source_span` | 语义层从 SWC span 取 | 编码进 WASM custom section，供运行时错误映射行列 |

这些字段的共同点：**信息只在语义层可得，但只在后端有用**。IR 是它们唯一的传递通道，所以字段留在 IR 而不是某一侧的私有结构里。

`known_callee_vars` 为空表示「不调用任何已知函数声明」，后端据此对未知 callee 保守地当作 may-GC。这是有意的保守方向：漏填只损失优化，误填会破坏 GC 正确性。

## 深入了解

- [IR 阶段在流水线中的位置](../pipeline/ir.md)
- [后端如何消费这些元数据做活跃性分析](../backend/liveness-slots-and-spills.md)
