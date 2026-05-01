# wjsm

一个采用 AOT 编译架构的 JavaScript/TypeScript 运行时，将 JS/TS 代码预编译为 WebAssembly 执行。

## 项目文档

完整项目文档请参阅 [PROJECT_GUIDE.md](./PROJECT_GUIDE.md)，包含：
- 技术架构详解
- 核心概念（IR 设计、NaN-boxing、作用域分析）
- 功能现状
- 开发指南

## 快速开始

### 编译

```bash
cargo build --release
```

### 使用

```bash
# 编译 JS/TS 到 WASM
./target/release/wjsm build test.ts -o out.wasm

# 直接运行
./target/release/wjsm run test.ts
```

## 示例

```typescript
console.log("Hello, wjsm!");
console.log(1 + 2 * 3);
```

```bash
$ ./target/release/wjsm run test.ts
Hello, wjsm!
7
```

## Roadmap

### 语言特性

| 类别 | 特性 | 状态 |
|------|------|------|
| 表达式 | 算术、位运算、比较、逻辑运算符 | ✅ 已完成 |
| 表达式 | 三元、逗号、复合赋值、自增自减 | ✅ 已完成 |
| 声明 | var/let/const、块级作用域、TDZ | ✅ 已完成 |
| 控制流 | if/else、switch、while/for/do-while | ✅ 已完成 |
| 控制流 | for...in/for...of、break/continue | ✅ 已完成 |
| 控制流 | try/catch/finally、throw | ✅ 已完成 |
| 函数/类 | 函数声明/表达式、箭头函数 | ✅ 已完成 |
| 函数/类 | class 声明、new 表达式、prototype | ✅ 已完成 |
| 对象 | 对象字面量、属性访问、in/instanceof | ✅ 已完成 |
| 对象 | Object.defineProperty/getOwnPropertyDescriptor | ✅ 已完成 |
| 字面量 | BigInt、正则、模板字符串、数组 | 🔲 计划中 |
| 高级 | 解构、async/await、装饰器 | 🔲 计划中 |

### 运行时

| 特性 | 状态 |
|------|------|
| 基础堆分配（bump allocator） | ✅ 已完成 |
| 原型链遍历 | ✅ 已完成 |
| 宿主函数（console.log 等） | ✅ 已完成 |
| 完整 GC | 🔲 计划中 |
| 闭包/词法作用域 | 🔲 计划中 |
| 模块系统（ESM/CJS） | 🔲 计划中 |

### 后端

| 特性 | 状态 |
|------|------|
| WASM 后端（wasmtime） | ✅ 已完成 |
| JIT 后端 | 🔲 计划中 |
| 多运行时支持 | 🔲 计划中 |

---

✅ = 已完成 | 🔲 = 计划中

## 许可证

[待添加]
