# wjsm

一个兼容 Node.js 语法的 JavaScript/TypeScript 运行时，采用 AOT（Ahead-of-Time）编译架构。

## 🎯 项目愿景

类似于 [Deno](https://deno.land/) 和 [Bun](https://bun.sh/)，wjsm 致力于成为一个高性能的 JavaScript/TypeScript 运行时。但与它们不同的是，**wjsm 不直接使用 V8 等 JS 引擎来解释执行代码**，而是：

1. **AOT 编译**：将 JS/TS 代码（ECMAScript）编译为 WebAssembly 模块
2. **多运行时支持**：支持多种 WASM 运行时执行编译后的代码（如 [wasmtime](https://wasmtime.dev/)、V8 的 WASM 支持等）
3. **原生性能**：通过 WASM 的接近原生性能，同时保持 JS/TS 的开发体验

## ✨ 核心特性

- 📦 **AOT 编译**：将 JS/TS 预编译为 WASM，实现更快的启动时间和可预测的内存使用
- 🔵 **TypeScript 一流支持**：使用 [SWC](https://swc.rs/) 解析器，原生支持 TS 语法，无需额外的转译步骤
- 🚀 **多 WASM 运行时兼容**：计划支持 wasmtime、Wasmer、V8 等多种 WASM 运行时
- 🔒 **沙箱安全**：利用 WASM 的内存安全特性，提供开箱即用的安全沙箱
- 📦 **Node.js 兼容**：目标是兼容常用的 Node.js API 和模块系统

## 🏗️ 架构

```
┌─────────────────┐
│   JS/TS 源代码   │
│  (Node.js 风格)  │
└────────┬────────┘
         │
         ▼
┌─────────────────┐
│   SWC Parser    │  ← 解析为 AST
└────────┬────────┘
         │
         ▼
┌─────────────────┐
│  wasm-encoder   │  ← 生成 WASM 字节码
│   (Codegen)     │
└────────┬────────┘
         │
         ▼
┌─────────────────┐
│    WASM 模块     │
└────────┬────────┘
         │
    ┌────┴────┐
    ▼         ▼
┌────────┐ ┌────────┐
│wasmtime│ │  V8    │  ← 多种运行时支持（计划中）
│(当前)  │ │(计划中)│
└────────┘ └────────┘
```

### 当前实现

- **Parser**: 使用 `swc_core` 解析 JS/TS AST
- **Codegen**: 使用 `wasm-encoder` 生成 WASM 字节码
- **Runtime**: 使用 `wasmtime` 执行 WASM，提供 `console.log` 等宿主函数
- **Value Encoding**: 采用 NaN boxing 技术编码 JS 值类型

## 🚀 快速开始

### 构建项目

```bash
# 克隆仓库
git clone <repo-url>
cd wjsm

# 构建
cargo build --release
```

### 使用 CLI

```bash
# 编译 JS/TS 文件到 WASM
./target/release/wjsm build test.ts -o out.wasm

# 直接运行 JS/TS 文件（编译并执行）
./target/release/wjsm run test.ts

# 使用 cargo 运行
cargo run -- build test.ts -o out.wasm
cargo run -- run test.ts
```

### 示例代码

```typescript
// test.ts
console.log("Hello World");
console.log(1 + 2 * 3);
```

运行结果：
```
Hello World
7
```

## 🛠️ 技术栈

- **Rust 2024 Edition**: 项目主体语言
- **SWC**: 高速 JavaScript/TypeScript 解析器
- **wasm-encoder**: WASM 模块编码器
- **wasmtime**: WebAssembly 运行时（当前默认）
- **anyhow/thiserror**: 错误处理
- **clap**: CLI 框架

## 📋 开发路线图

### 当前阶段（PoC）
- [x] 基础 AOT 编译流程
- [x] `console.log` 支持
- [x] 基本算术运算
- [x] 字符串字面量

### 短期目标
- [ ] 完整的 JavaScript 表达式支持
- [ ] 变量声明和作用域
- [ ] 控制流（if/else, loop）
- [ ] 函数定义和调用
- [ ] 更多 Node.js API 兼容

### 长期目标
- [ ] 模块化系统（ES Modules / CommonJS）
- [ ] 多 WASM 运行时支持（wasmtime, wasmer, V8）
- [ ] 标准库实现（fs, path, http 等）
- [ ] npm 包兼容性
- [ ] 性能优化和 JIT 支持

## 🤝 贡献

欢迎提交 Issue 和 PR！

## 📄 许可证

[待添加]
