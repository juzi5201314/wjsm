# wjsm IR v1 design

## 目标

Era 0-2 的 IR 只服务当前 PoC 能力的硬切换：

```text
parse -> semantic/lowering -> IR -> backend-wasm -> runtime
```

这个版本不追求“最终优化 IR”，只要求：

1. 与 SWC AST 解耦，不再把 AST 当成 backend 输入契约。
2. 明确表达 lowered 结果，而不是把语义逻辑散落在 backend 里。
3. 让 backend-wasm 与后续 backend-jit 都能共享同一份输入结构。
4. 以文本 dump 作为稳定快照格式，给 Era 2 的 parity 工作提供证据。

## 结构

### Module

- `constants: Vec<Constant>`
- `functions: Vec<Function>`

`Module` 是整个编译单元的根。当前 PoC 只有一个入口函数 `main`，但结构上已经允许未来扩展多个 lowered function。

### Function

- `name`
- `entry: BasicBlockId`
- `blocks: Vec<BasicBlock>`

函数显式记录入口块，避免把“第一个 block 就是入口”这种约定写死在 backend 里。

### BasicBlock

- `id`
- `instructions`
- `terminator`

即便当前 PoC 只有单块直线代码，也强制每个 block 拥有显式 terminator。这样 Era 3 以后引入分支/跳转时，不需要再改写 IR 基本骨架。

### Instruction

当前冻结三类指令：

1. `Const`
   - 从 constant pool 读取常量，产出 `ValueId`
2. `Binary`
   - `add/sub/mul/div`
3. `CallBuiltin`
   - 目前只支持 `builtin.console.log`

### Terminator

- `return`
- `return %value`

当前主线使用 `return`，保留带返回值的形态是为了避免未来再改 terminator 结构。

## Constant pool

当前 constant pool 支持：

- `number(f64)`
- `string(String)`

设计约束：

1. lowering 阶段把字面量统一放进 constant pool。
2. backend 通过 `ConstantId` 访问常量，而不是回看 AST。
3. 文本 dump 中常量 id 稳定显示为 `cN`。

当前版本不做常量去重；是否 dedup 留给后续优化阶段决定。

## Builtin handles

当前只冻结：

- `Builtin::ConsoleLog`

原因：

1. 这是现有 PoC 唯一宿主调用。
2. 先把 builtin call 从“特判某个 AST 形状”提升为 IR 层显式节点。
3. 后续 host ABI 统一时，可以把 `Builtin` 扩展成更正式的 host symbol / intrinsic handle。

## Textual dump format

IR dump 目标是：

1. 稳定、可快照
2. 人眼可读
3. 不依赖 AST pretty-printer

示例：

```text
module {
  constants:
    c0 = number(1)
    c1 = number(2)
    c2 = number(3)

  fn @main [entry=bb0]:
    bb0:
      %0 = const c0
      %1 = const c1
      %2 = const c2
      %3 = mul %1, %2
      %4 = add %0, %3
      call builtin.console.log(%4)
      return
}
```

约定：

- 常量 id：`cN`
- block id：`bbN`
- SSA-like value id：`%N`
- builtin 调用：`call builtin.console.log(...)`

## Lowering 边界

Era 2 的 lowering 只覆盖：

1. 数字字面量
2. 字符串字面量
3. `+ - * /`
4. `console.log(expr)`
5. 语义错误最小 diagnostics 映射

这意味着当前 IR **不会** 表达：

- 变量 / 绑定
- 控制流
- 对象 / 数组 / 属性访问
- 函数定义 / 闭包
- 异常 / async / module evaluation

## Deferred features

以下内容明确延后，不在 IR v1 / Era 2 范围内：

1. typed high-level JS semantic IR
2. phi / branch / jump / CFG 优化工具
3. symbol table / scope object lowering
4. object model / heap references / GC handles
5. source map rich diagnostics
6. optimizer pipeline
7. JIT-specific annotations

## Non-goals

这个版本 **不是**：

- 通用 JavaScript IR
- 最终 runtime contract
- 优化器输入格式
- SSA 完整规范实现

它只是 Era 2 的正式输入契约：足够表达当前 PoC，且能把 AST 从 backend 主线里切掉。
