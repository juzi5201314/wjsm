# wjsm 项目完全指南

> 一份完整、通俗的项目介绍文档，涵盖项目规范、结构、执行流程和技术选型。

---

## 目录

1. [项目概述](#1-项目概述)
2. [技术架构](#2-技术架构)
3. [目录结构](#3-目录结构)
4. [快速开始](#4-快速开始)
5. [核心概念](#5-核心概念)
6. [技术栈详解](#6-技术栈详解)
7. [功能现状](#7-功能现状)
8. [开发指南](#8-开发指南)
9. [常见问题](#9-常见问题)

---

## 1. 项目概述

### 1.1 项目定位

**wjsm** 是一个实验性的 JavaScript/TypeScript 运行时，采用 **AOT（Ahead-of-Time）编译** 架构。与传统的 JavaScript 引擎（如 V8、SpiderMonkey）不同，wjsm 不解释执行 JavaScript 代码，而是将 JS/TS 代码预先编译为 WebAssembly 模块，然后由 WASM 运行时执行。

### 1.2 项目愿景

类似于 [Deno](https://deno.land/) 和 [Bun](https://bun.sh/)，wjsm 致力于成为高性能的 JavaScript/TypeScript 运行时。但与它们的核心区别在于：

| 特性 | 传统 JS 引擎 (V8/Bun) | wjsm |
|------|----------------------|------|
| 执行方式 | JIT 即时编译 + 解释执行 | AOT 预编译 + WASM 执行 |
| 启动速度 | 较慢（需编译） | 极快（无编译开销） |
| 内存使用 | JIT 编译器占用额外内存 | 可预测的静态内存 |
| 安全性 | 依赖宿主环境 | WASM 沙箱天然隔离 |
| 多运行时 | 仅单一引擎 | 理论上可切换运行时 |

### 1.3 核心价值

- **🚀 极速启动**：AOT 编译消除了运行时的解析和编译开销
- **🔒 天然沙箱**：WASM 内存安全特性提供开箱即用的安全隔离
- **📦 格式紧凑**：WASM 二进制格式体积小、传输效率高
- **🌐 多运行时**：编译产物可在不同 WASM 运行时间移植
- **🛠️ TypeScript 一等公民**：使用 SWC 解析器，原生支持 TS 语法

---

## 2. 技术架构

### 2.1 编译流水线

wjsm 采用**线性编译流水线**，每个阶段产生下一阶段的输入：

```
┌─────────────────────────────────────────────────────────────────────────┐
│                         wjsm 编译流水线                                 │
└─────────────────────────────────────────────────────────────────────────┘

     ┌──────────────┐     ┌──────────────┐     ┌──────────────┐     ┌──────────────┐
     │  JavaScript  │     │     AST      │     │      IR      │     │   WebAssembly │
     │   源代码      │ ──▶ │  (SWC 解析)   │ ──▶ │  (中间表示)   │ ──▶ │    字节码      │
     │  .js / .ts   │     │              │     │              │     │              │
     └──────────────┘     └──────────────┘     └──────────────┘     └──────────────┘
                               │                    │                    │
                               ▼                    ▼                    ▼
                        ┌──────────────┐     ┌──────────────┐     ┌──────────────┐
                        │ wjsm-parser │     │ wjsm-ir      │     │ wjsm-backend │
                        │ crates/     │     │              │     │ -wasm        │
                        └──────────────┘     └──────────────┘     └──────────────┘
                                                                          │
                                                    ┌─────────────────────┤
                                                    ▼                     ▼
                                             ┌──────────────┐     ┌──────────────┐
                                             │   wasmtime   │     │    (其他)     │
                                             │   (当前)      │     │  (计划中)     │
                                             └──────────────┘     └──────────────┘
```

### 2.2 模块职责

项目采用 Cargo Workspace 结构，每个 crate 职责单一：

| Crate | 职责 | 输入 | 输出 | 外部依赖 |
|-------|------|------|------|----------|
| **wjsm-parser** | 解析器：将 JS/TS 源码解析为 AST | 源代码字符串 | `swc_ast::Module` | swc_core |
| **wjsm-semantic** | 语义分析：AST 降级、作用域分析、错误诊断 | SWC AST | `wjsm_ir::Program` | swc_core, wjsm-ir |
| **wjsm-ir** | 中间表示：无外部依赖的内部 IR 定义 | - | IR 数据结构 | 无 |
| **wjsm-backend-wasm** | WASM 后端：IR 编译为 WASM 字节码 | `&Program` | `Vec<u8>` (WASM) | wasm-encoder, wjsm-ir |
| **wjsm-backend-jit** | JIT 后端：占位 stub | `&Program` | `Vec<u8>` | wjsm-ir |
| **wjsm-runtime** | 运行时：wasmtime 执行、宿主函数 | WASM 字节码 | 执行结果 | wasmtime, wjsm-ir |
| **wjsm-cli** | CLI：命令行入口，编排各模块 | 命令行参数 | - | 所有模块 + clap |

### 2.3 数据流详解

```
用户输入: test.ts
    │
    ▼
┌────────────────────────────────────────────────────────────────────────┐
│ 1. 解析 (wjsm-parser)                                                   │
│    - 调用 swc_core 解析 JS/TS                                           │
│    - 输出: swc_ast::Module                                              │
└────────────────────────────────────────────────────────────────────────┘
    │
    ▼
┌────────────────────────────────────────────────────────────────────────┐
│ 2. 语义降级 (wjsm-semantic)                                             │
│    - 变量声明处理 (var 提升、let/const TDZ)                              │
│    - 作用域链构建                                                        │
│    - 生成 IR 指令                                                        │
│    - 输出: wjsm_ir::Program                                             │
└────────────────────────────────────────────────────────────────────────┘
    │
    ▼
┌────────────────────────────────────────────────────────────────────────┐
│ 3. 代码生成 (wjsm-backend-wasm)                                         │
│    - 遍历 IR 指令                                                       │
│    - 转换为 WASM 字节码                                                 │
│    - 嵌入常量池（字符串、数字）                                          │
│    - 输出: WASM 二进制 (.wasm 文件)                                     │
└────────────────────────────────────────────────────────────────────────┘
    │
    ▼
┌────────────────────────────────────────────────────────────────────────┐
│ 4. 执行 (wjsm-runtime)                                                   │
│    - wasmtime 加载 WASM 模块                                             │
│    - 链接宿主函数 (console.log 等)                                       │
│    - 调用 main() 导出函数                                               │
│    - 输出: 程序执行结果                                                  │
└────────────────────────────────────────────────────────────────────────┘
```

---

## 3. 目录结构

```
wjsm/
├── src/                          # 工作区根入口
│   └── main.rs                   # 2行代码：调用 wjsm_cli::main_entry()
│
├── crates/                       # 所有 crate 源码
│   ├── wjsm-parser/             # JS/TS 解析器
│   │   ├── src/lib.rs
│   │   └── Cargo.toml
│   ├── wjsm-semantic/           # 语义分析与 IR 降级
│   │   ├── src/lib.rs
│   │   ├── tests/lowering_snapshots.rs
│   │   └── Cargo.toml
│   ├── wjsm-ir/                 # 中间表示定义
│   │   ├── src/
│   │   │   ├── lib.rs          # Module, Function, BasicBlock, Instruction
│   │   │   ├── value.rs        # NaN-boxing 值编码
│   │   │   └── constants.rs     # 属性偏移量等常量
│   │   ├── docs/ir-design.md   # IR 设计文档
│   │   ├── tests/ir_dump.rs
│   │   └── Cargo.toml
│   ├── wjsm-backend-wasm/        # WASM 代码生成器
│   │   ├── src/lib.rs
│   │   └── Cargo.toml
│   ├── wjsm-backend-jit/         # JIT 后端（stub）
│   │   └── Cargo.toml
│   ├── wjsm-runtime/             # wasmtime 运行时
│   │   ├── src/lib.rs           # 宿主函数定义
│   │   └── Cargo.toml
│   └── wjsm-cli/                # 命令行工具
│       ├── src/
│       │   ├── lib.rs          # CLI 逻辑
│       │   └── main.rs         # 二进制入口
│       └── Cargo.toml
│
├── fixtures/                    # 测试用例
│   ├── happy/                   # 成功路径用例 (*.js + *.expected)
│   ├── errors/                  # 错误路径用例 (*.js + *.expected)
│   ├── semantic/                # IR 快照 (*.ir)
│   └── modules/                 # 模块系统用例（待实现）
│
├── tests/                       # 测试代码
│   ├── integration/             # E2E 集成测试
│   ├── unit/                   # 单元测试
│   └── fixture_runner.rs       # 测试运行器
│
├── .config/                     # 配置
│   └── nextest.toml            # Nextest 测试配置
│
├── Cargo.toml                   # 工作区根配置
├── Cargo.lock
├── README.md                    # 项目简介
├── AGENTS.md                    # AI 开发规范
└── todo.md                      # 待实现功能清单
```

### 关键目录说明

| 目录 | 用途 |
|------|------|
| `crates/wjsm-ir/src/` | IR 类型定义、NaN-boxing 编码实现 |
| `crates/wjsm-ir/docs/` | IR 设计文档（中文） |
| `fixtures/` | 快照测试用例，包含输入和预期输出 |
| `tests/` | 集成测试和单元测试代码 |
| `.config/` | 开发工具配置（nextest 等） |

---

## 4. 快速开始

### 4.1 环境要求

- **Rust 2024 Edition**
- Cargo (通常随 Rust 安装)

### 4.2 构建项目

```bash
# 克隆仓库
git clone <repo-url>
cd wjsm

# Debug 构建
cargo build

# Release 构建（推荐用于生产）
cargo build --release
```

### 4.3 使用 CLI

```bash
# 编译 JS/TS 文件到 WASM
cargo run -- build test.ts -o out.wasm

# 或使用 release 构建
./target/release/wjsm build test.ts -o out.wasm

# 直接运行 JS/TS 文件（编译并执行）
cargo run -- run test.ts

# 查看帮助
cargo run -- --help
```

### 4.4 运行测试

```bash
# 运行所有测试
cargo test

# 运行特定 crate 的测试
cargo test -p wjsm-semantic

# 使用 nextest（更快）
cargo install cargo-nextest
cargo nextest run

# 更新快照测试
WJSM_UPDATE_FIXTURES=1 cargo test
```

### 4.5 示例

创建 `test.ts`:

```typescript
// test.ts
console.log("Hello, wjsm!");
console.log("1 + 2 * 3 =", 1 + 2 * 3);

let x = 10;
if (x > 5) {
    console.log("x is greater than 5");
}

for (let i = 0; i < 3; i++) {
    console.log("Loop:", i);
}
```

运行:

```bash
cargo run -- run test.ts
```

输出:

```
Hello, wjsm!
1 + 2 * 3 = 7
x is greater than 5
Loop: 0
Loop: 1
Loop: 2
```

---

## 5. 核心概念

### 5.1 中间表示 (IR) 设计

wjsm 使用自定义的中间表示（IR）作为编译流水线的核心数据结构。IR 设计的核心目标：

1. **解耦**：与 SWC AST 解耦，backend 不直接依赖特定解析器
2. **表达力**：明确表达语义降级结果，而非散落在 backend 中
3. **可共享**：IR 是 backend-wasm 和 backend-jit 的共同输入
4. **可测试**：文本 dump 作为稳定快照格式

#### IR 结构

```text
module {
  constants:                     # 常量池
    c0 = number(1.0)
    c1 = string("Hello")

  fn @main [entry=bb0]:         # 函数定义
    bb0:                         # 基本块
      %0 = const c0              # SSA 形式指令
      %1 = const c1
      call builtin.console.log(%0)
      return
}
```

#### IR 组件

| 组件 | 说明 |
|------|------|
| **Module** | 编译单元根节点，包含常量和函数列表 |
| **Function** | 函数，包含名称、参数、入口块、块列表 |
| **BasicBlock** | 基本块，包含指令序列和终止符 |
| **Instruction** | SSA 形式指令，如 Const, Binary, CallBuiltin |
| **Terminator** | 终止符，如 Return, Jump, Branch |
| **Constant** | 常量，如 Number, String, Bool |

#### 支持的指令类型

- **Const**: 从常量池加载常量
- **Binary**: 算术运算 (add, sub, mul, div, mod, exp, 位运算)
- **Unary**: 一元运算 (not, neg, pos, bitnot, void)
- **Compare**: 比较运算 (eq, neq, stricteq, lt, lteq, gt, gteq)
- **Phi**: SSA φ 节点，用于控制流合并
- **CallBuiltin**: 调用内置函数 (console.log, typeof 等)
- **LoadVar / StoreVar**: 变量加载/存储
- **Call**: 函数调用
- **NewObject**: 创建对象
- **GetProp / SetProp**: 属性访问
- **DeleteProp**: 属性删除
- **SetProto**: 原型链设置

### 5.2 NaN-Boxing 值编码

JavaScript 是动态类型语言，一个变量可能是数字、字符串、布尔、对象等。wjsm 使用 **NaN-boxing** 技术在 64 位整数中编码所有 JS 值类型。

#### 编码格式

```
┌──────────────────────────────────────────────────────────────────┐
│                        64-bit i64                                │
├──────────┬────────────┬──────────────────────────────────────────┤
│  Tag     │  Reserved  │  Payload                                  │
│ (3 bits) │  (29 bits) │  (32 bits)                               │
└──────────┴────────────┴──────────────────────────────────────────┘
```

#### 值类型编码

| 类型 | 编码方式 |
|------|----------|
| **f64 数字** | 直接存储 IEEE 754 编码（高位非全1） |
| **字符串指针** | tag=1, payload=指针地址 |
| **未定义** | tag=2, payload=0 |
| **布尔** | tag=3, payload=0(false)/1(true) |
| **空指针** | tag=3, payload=2 (null) |
| **对象句柄** | tag=4, payload=句柄索引 |
| **函数引用** | tag=5, payload=函数索引 |
| **异常** | tag=6, payload=异常索引 |
| **迭代器** | tag=7, payload=迭代器索引 |
| **枚举器** | tag=8, payload=枚举器索引 |

#### 为什么要用 NaN-Boxing？

- **空间效率**：只需 8 字节即可存储所有 JS 值类型
- **类型区分**：通过不同的位模式区分类型
- **WASM 兼容**：所有值都编码为 i64，可直接在 WASM 中传递

### 5.3 作用域与变量提升

JavaScript 有独特的作用域规则，wjsm 的语义分析层正确处理了这些规则。

#### 变量声明处理

| 声明类型 | 处理方式 |
|----------|----------|
| **var** | 函数作用域声明，变量提升到函数顶部，初始值为 undefined |
| **let** | 块级作用域，存在 TDZ（暂时性死区），访问前必须赋值 |
| **const** | 块级作用域，类似 let，但必须初始化且不可重新赋值 |

#### 两阶段 Lowering

wjsm-semantic 使用两阶段处理来正确实现变量提升和 TDZ：

```
阶段 1 (Pre-declare):
  - 遍历所有声明，创建作用域树
  - var 声明提升到函数作用域，初始化为 undefined
  - let/const 注册到块级作用域，标记为未初始化（TDZ）

阶段 2 (Lower):
  - 遍历 AST，生成 IR 指令
  - 访问变量时检查 TDZ 状态
  - StoreVar/LoadVar 操作正确的作用域
```

#### 作用域树

```rust
struct ScopeTree {
    root: ScopeId,
    scopes: Vec<Scope>,
}

struct Scope {
    kind: ScopeKind,      // Block 或 Function
    vars: HashMap<String, VarInfo>,
    parent: Option<ScopeId>,
}

enum VarKind {
    Var,   // var 声明
    Let,   // let 声明
    Const, // const 声明
}
```

---

## 6. 技术栈详解

### 6.1 Rust

**选择理由**：

- **内存安全**：编译时保证内存安全，减少运行时错误
- **零成本抽象**：高性能，适合系统编程
- **WASM 友好**：wasm-bindgen、wasm-pack 等成熟工具
- **表达力强**：模式匹配、类型系统有助于复杂逻辑实现

**项目配置**：

```toml
edition = "2024"  # Rust 2024 Edition
resolver = "2"    # Cargo 依赖解析器 v2
```

### 6.2 SWC

**选择理由**：

- **极速解析**：比 Babel 快 20-100 倍
- **TypeScript 原生支持**：无需额外转译
- **稳定可靠**：经过大量生产项目验证
- **API 友好**：提供清晰的 AST 结构

**使用方式**：

```rust
use swc_core::{
    ecma::parser::{lexer::Lexer, Parser, StringInput, Syntax, TsSyntax},
    ecma::ast::Module,
};

pub fn parse_module(source: &str) -> Result<Module> {
    let syntax = Syntax::Typescript(TsSyntax {
        ..Default::default()
    });
    let lexer = Lexer::new(syntax, Default::default(), StringInput::from(source), None);
    let mut parser = Parser::new_from(lexer);
    Ok(parser.parse_module()?)
}
```

### 6.3 wasm-encoder

**选择理由**：

- **类型安全**：纯 Rust 实现的 WASM 字节码编码
- **无 C 依赖**：不依赖 LLVM/Clang
- **活跃维护**：与 wasmparser 形成互补生态

**使用方式**：

```rust
use wasm_encoder::{Function, Module, TypeSection, CodeSection};

let mut module = Module::new();
let mut types = TypeSection::new();
types.func(vec![ValType::I64], vec![]);
module.section(&types);
// ... 添加函数、内存、导出等
let bytes = module.finish();
```

### 6.4 wasmtime

**选择理由**：

- **高性能**：Cranelift JIT 编译
- **符合标准**：完全支持 WASM 标准
- **嵌入式友好**：易于集成到 Rust 应用
- **安全沙箱**：WASM 的内存隔离特性

**使用方式**：

```rust
use wasmtime::{Engine, Module, Store, Instance};

let engine = Engine::default();
let module = Module::new(&engine, wasm_bytes)?;
let mut store = Store::new(&engine, ());
let instance = Instance::new(&mut store, &module, &imports)?;
let main = instance.get_typed_func::<(), ()>(&mut store, "main")?;
main.call(&mut store, ())?;
```

### 6.5 其他依赖

| 库 | 用途 |
|----|------|
| **anyhow** | 通用错误处理，`Result<T>` 简化 |
| **thiserror** | 领域错误定义，`enum` + derive 模式 |
| **clap** | CLI 参数解析，支持子命令 |

---

## 7. 功能现状

### 7.1 已实现功能 ✅

#### 表达式与运算

- [x] 数字、字符串、布尔、空值字面量
- [x] 算术运算：`+`, `-`, `*`, `/`, `%`, `**`
- [x] 位运算：`|`, `&`, `^`, `<<`, `>>`, `>>>`
- [x] 比较运算：`==`, `!=`, `===`, `!==`, `<`, `<=`, `>`, `>=`
- [x] 逻辑运算：`&&`, `||`, `??`
- [x] 一元运算：`!`, `-`, `+`, `~`, `typeof`, `void`
- [x] 复合赋值：`+=`, `-=`, `*=`, `/=`, `%=`, `**=`, `<<=`, `>>=`, `>>>=`, `&=`, `|=`, `^=`
- [x] 自增自减：`++`, `--`
- [x] 三元表达式：`a ? b : c`
- [x] 逗号表达式：`a, b`

#### 声明与作用域

- [x] `var` 声明与提升
- [x] `let` / `const` 声明
- [x] 块级作用域
- [x] TDZ（暂时性死区）检查

#### 控制流

- [x] `if` / `else` 条件语句
- [x] `switch` 语句（含 default, fallthrough）
- [x] `while` / `do...while` 循环
- [x] `for` 循环
- [x] `for...in` / `for...of` 迭代
- [x] `break` / `continue`（含 label）
- [x] `return` 语句
- [x] `try` / `catch` / `finally`
- [x] `throw` 异常

#### 函数与类

- [x] 函数声明与表达式
- [x] 箭头函数
- [x] `class` 声明（含构造函数、方法）
- [x] `new` 表达式
- [x] `this` 关键字
- [x] 原型链

#### 对象

- [x] 对象字面量
- [x] 属性访问：`obj.prop`, `obj["prop"]`
- [x] 属性赋值与删除
- [x] `in` / `instanceof` 运算符
- [x] `Object.defineProperty()`
- [x] `Object.getOwnPropertyDescriptor()`

### 7.2 待实现功能 ❌

#### 字面量与数据结构

- [ ] `BigInt`
- [ ] 正则表达式 `/.../`
- [ ] 模板字符串 `` `hello ${world}` ``
- [ ] 数组字面量 `[1, 2, 3]`
- [ ] 数组方法（`push`, `map`, `filter` 等）
- [ ] JSX

#### 运行时

- [ ] 完整 GC（当前仅简单 bump allocator）
- [ ] 闭包（词法变量捕获）
- [ ] 更多宿主 API（`console.error`, `setTimeout`, `fetch` 等）

#### 高级特性

- [ ] 模块系统（ES Modules / CommonJS）
- [ ] `super` 关键字
- [ ] 可选链 `a?.b`
- [ ] 解构赋值
- [ ] 异步/等待

#### JIT 后端

- [ ] `wjsm-backend-jit` 当前为 stub

---

## 8. 开发指南

### 8.1 添加新语言特性

添加新的 JavaScript 特性通常涉及以下步骤：

#### 步骤 1：扩展 IR（wjsm-ir）

在 `crates/wjsm-ir/src/lib.rs` 中添加新的指令类型：

```rust
#[derive(Debug, Clone, PartialEq)]
pub enum Instruction {
    // ... 现有指令

    // 添加新指令
    YourNewInstruction {
        dest: ValueId,
        // 其他参数...
    },
}
```

#### 步骤 2：更新语义分析（wjsm-semantic）

在 `crates/wjsm-semantic/src/lib.rs` 中处理新的 AST 节点：

```rust
fn lower_your_node(&mut self, node: &YourAstNode) -> LoweringResult {
    // 生成对应的 IR 指令
    let dest = self.emit_instr(Instruction::YourNewInstruction {
        dest: self.next_value(),
        // 参数...
    });
    Ok(dest)
}
```

#### 步骤 3：更新 WASM 后端（wjsm-backend-wasm）

在 `crates/wjsm-backend-wasm/src/lib.rs` 中生成 WASM 字节码：

```rust
fn compile_instruction(&mut self, instr: &Instruction) -> WasmResult<()> {
    match instr {
        Instruction::YourNewInstruction { dest, .. } => {
            // 生成 WASM 指令
            self.emit(0x20, 0x00); // 示例
            self.assign_local(dest);
        }
        // ...
    }
    Ok(())
}
```

#### 步骤 4：更新运行时（wjsm-runtime）

如有需要，在 `crates/wjsm-runtime/src/lib.rs` 中添加新的宿主函数：

```rust
let your_fn = Func::wrap(&mut store, |caller: Caller, arg: i64| -> i64 {
    // 实现逻辑
    value::encode_undefined()
});
```

#### 步骤 5：添加测试

1. 在 `fixtures/happy/` 添加 `.js` 测试文件
2. 创建对应的 `.expected` 快照文件
3. 运行 `WJSM_UPDATE_FIXTURES=1 cargo test` 生成快照

### 8.2 测试策略

wjsm 采用三层测试策略：

#### 1. IR 单元测试 (`wjsm-ir/tests/`)

测试 IR 结构的序列化和 dump 格式：

```rust
#[test]
fn test_ir_dump() {
    let mut module = Module::new();
    module.add_constant(Constant::Number(42.0));
    // ...
    let dump = module.dump_text();
    assert_snapshot!(dump);
}
```

#### 2. 语义快照测试 (`wjsm-semantic/tests/`)

测试 JS 源码到 IR 的转换：

```rust
#[test]
fn hello_fixture_matches_ir_snapshot() {
    let source = fs::read_to_string("fixtures/hello.js").unwrap();
    let module = wjsm_parser::parse_module(&source).unwrap();
    let program = wjsm_semantic::lower_module(module).unwrap();
    let dump = program.dump_text();
    assert_snapshot!("hello", dump);
}
```

#### 3. E2E 集成测试 (`tests/`)

测试完整的编译-执行流程：

```bash
# 运行所有 fixture 测试
cargo test --test integration

# 更新快照
WJSM_UPDATE_FIXTURES=1 cargo test
```

### 8.3 代码规范

| 规范 | 说明 |
|------|------|
| **Rust Edition** | 2024 Edition |
| **命名** | 类型用 UpperCamelCase，函数/变量用 snake_case |
| **注释语言** | 代码注释使用中文 |
| **错误处理** | CLI/运行时用 `anyhow::Result`，语义分析用 `thiserror` |
| **API 设计** | 每个 crate 暴露 1-2 个公共函数 |

### 8.4 常用命令

```bash
# 构建
cargo build --release

# 测试
cargo test
cargo test -p <crate-name>
cargo nextest run

# 检查
cargo check
cargo clippy

# 运行
cargo run -- build test.ts -o out.wasm
cargo run -- run test.ts
```

---

## 9. 常见问题

### Q: wjsm 与 Deno/Bun 有什么区别？

**A**: 主要区别在于执行方式。wjsm 使用 AOT 预编译，而 Deno/Bun 使用 JIT 即时编译。AOT 编译的优势是启动速度更快、内存使用更可预测；劣势是缺乏运行时优化的机会。

### Q: 为什么选择 Rust？

**A**: Rust 提供了内存安全保证、零成本抽象、优秀的 WASM 支持，以及丰富的工具链。对于需要高性能且安全可靠的编译器/Runtime 项目，Rust 是理想选择。

### Q: wjsm 能替代 Node.js 吗？

**A**: 目前不能。wjsm 仍处于早期阶段（PoC），许多 Node.js API、npm 生态兼容、模块系统等尚未实现。它的定位是一个实验性项目，探索 AOT + WASM 的技术路线。

### Q: 什么是 NaN-boxing？为什么用它？

**A**: NaN-boxing 是一种在 64 位浮点数表示中编码多种 JavaScript 值类型的技术。由于 IEEE 754 浮点数的特殊编码，大量位模式表示 NaN（非数字），我们利用这些空闲位来存储其他类型的值。这使得所有 JS 值都能用 8 字节的 i64 表示。

### Q: 如何参与贡献？

**A**: 请参考 `todo.md` 中的待实现功能列表，选择感兴趣的功能进行开发。提交前请确保：
1. 添加对应的测试用例
2. 运行 `cargo test` 确保通过
3. 遵循代码规范

### Q: 为什么 AGENTS.md 是英文的？

**A**: AGENTS.md 主要面向 AI 开发助手（如 GitNexus），使用英文便于工具理解。README.md 和其他文档使用中文，因为项目主力语言是中文。

---

## 附录

### A. 词汇表

| 术语 | 说明 |
|------|------|
| **AOT** | Ahead-of-Time，运行时前编译 |
| **JIT** | Just-In-Time，即时编译 |
| **IR** | Intermediate Representation，中间表示 |
| **SSA** | Static Single Assignment，静态单赋值 |
| **TDZ** | Temporal Dead Zone，暂时性死区 |
| **WASM** | WebAssembly，WebAssembly 二进制格式 |
| **NaN-boxing** | 利用 NaN 位模式编码多类型值的技术 |

### B. 相关资源

- [WASM 规范](https://webassembly.github.io/spec/)
- [wasmtime 文档](https://docs.wasmtime.dev/)
- [SWC 文档](https://swc.rs/docs/getting-started)
- [Rust 官方文档](https://doc.rust-lang.org/book/)

### C. 许可证

[待添加]

---

*本文档最后更新于 2024 年*
