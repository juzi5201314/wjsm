# Repository Guidelines

## Project Overview

`wjsm` 是一个兼容 Node.js 语法的 JavaScript/TypeScript 运行时，采用独特的 AOT（Ahead-of-Time）编译架构。

与 Deno/Bun 等运行时不同，wjsm **不直接使用 V8 等 JS 引擎解释执行**，而是：
- 将 JS/TS 代码 AOT 编译为 WebAssembly 模块
- 通过多种 WASM 运行时（wasmtime、V8 等）执行编译后的代码
- 实现高性能、低内存占用的 JS 执行环境

## Project Structure

```
wjsm/
├── src/
│   ├── main.rs           # CLI 入口，定义 build/run 命令
│   ├── runtime.rs        # WASM 运行时执行（当前使用 wasmtime）
│   └── compiler/
│       ├── mod.rs        # 编译器模块导出
│       ├── codegen.rs    # WASM 代码生成（swc AST → wasm-encoder）
│       └── value.rs      # JS 值类型编码/解码（NaN boxing）
├── Cargo.toml            # Rust 项目配置
├── test.ts               # 测试示例文件
└── README.md             # 项目文档
```

## Build Commands

```bash
# 构建项目
cargo build

# 发布构建
cargo build --release

# 编译 JS 文件到 WASM
cargo run -- build test.ts -o out.wasm

# 直接运行 JS 文件（编译并执行）
cargo run -- run test.ts
```

## Code Style

- 使用 Rust 2024 edition
- 依赖管理：anyhow, clap, swc_core, wasmtime, wasm-encoder
- 错误处理：优先使用 `anyhow::Result` 和 `thiserror`
- 代码注释使用中文，保持简洁清晰

## Architecture Details

### 编译流程
1. **解析**: `swc_core` 将 JS/TS 源码解析为 AST
2. **代码生成**: `wasm-encoder` 将 AST 转换为 WASM 字节码
3. **值编码**: 使用 NaN boxing 技术在 64 位浮点数中编码 JS 值类型
4. **执行**: `wasmtime` 加载并执行 WASM 模块

### 宿主函数
当前实现提供以下宿主函数供 WASM 模块调用：
- `env.console_log(val: i64)`: 输出日志，支持数字和字符串

### 支持的语法（当前 PoC 阶段）
- `console.log()` 调用
- 数字字面量和基本算术运算 (+, -, *, /)
- 字符串字面量

## Testing

暂无自动化测试套件。通过手动运行测试文件验证功能：

```bash
cargo run -- run test.ts
```

## Future Roadmap

### 短期目标
- 完整的 JavaScript 表达式支持
- 变量声明和作用域
- 控制流（if/else, loop）
- 函数定义和调用

### 长期目标
- 模块化系统（ES Modules / CommonJS）
- 多 WASM 运行时支持（wasmtime, wasmer, V8）
- 标准库实现（fs, path, http 等）
- npm 包兼容性

## Commit Guidelines

- `feat:` 新功能
- `fix:` 修复
- `docs:` 文档更新
- `refactor:` 重构
- 保持简洁清晰的提交信息
