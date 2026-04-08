# wjsm 生产级规划：通用 JS/TS 运行时

## TL;DR

> **目标**：将 wjsm 从 335 行 PoC 演进到与 Deno 同等成熟度的通用 JS/TS 运行时，支持完整 npm 生态和 AOT+JIT 混合架构。

> **核心挑战**：这是一个从零构建完整 JS 引擎 + TS 类型检查器 + 包管理器 + Node 兼容层的史诗级工程，预估 50,000+ 行代码，数年级别的长期项目。

> **架构决策**：
> - AOT+JIT 混合编译（动态特性当场 JIT）
> - 分层编译器（AST → Lowered IR → WASM/机器码）
> - ESM-first，CJS 适配层
> - 完整 TypeScript 类型检查
> - 完整 npm 兼容目标（含 Node-API）

> **关键路径**：测试基础设施 → 编译器分层 → 语义模型 → 值/堆模型 → 执行模型 → 模块系统 → CJS/npm 兼容 → 宿主 API → JIT → TS 类型检查 → 工具链 → 性能/安全硬化

> **并行执行**：可分 6 大阶段，每阶段内部高度并行，阶段间有依赖关系

## Context

### 原始请求
用户要求将 wjsm 从当前 PoC（仅支持 `console.log` + 简单算术）做到与 Deno 同等成熟度的生产级 JS/TS 运行时，支持全部 npm 包，AOT+JIT 混合架构。

### 关键决策（已确认）

| 决策点 | 选择 | 复杂度 |
|--------|------|--------|
| 定位 | 通用运行时（全功能，标准兼容） | 极高 |
| npm 兼容 | 支持全部 npm 包 | 极高 |
| 编译策略 | AOT+JIT 混合 | 极高 |
| 动态特性 | JIT 当场编译 | 极高 |
| TS 类型检查 | 完整类型检查 | 高 |
| 安全模型 | 功能优先（后期增强） | 中等 |
| 时间预期 | 无约束 | - |

### 当前现状（335 行代码）

**已实现**：
- CLI: `build`/`run` 命令
- 编译: SWC 解析 + wasm-encoder 直接生成 WASM
- 运行时: wasmtime 执行，仅 `console_log` 宿主函数
- 语言支持: 仅 `console.log(字面量/算术表达式)`

**缺失**（完整 Deno 级运行时所需）：
1. 变量、作用域、标识符解析
2. 控制流（if/else、循环、switch、try-catch）
3. 函数（声明、闭包、this、async/await、generator）
4. 对象模型（对象、数组、原型链、Proxy、Reflect）
5. 内存管理（堆分配、GC）
6. 模块系统（ESM/CJS、import/export、loader）
7. TypeScript 类型系统（完整类型检查）
8. JIT 编译器（动态代码即时编译）
9. AOT+JIT 协调（统一 IR、对象模型）
10. 标准库（fs/path/http/timers 等全兼容）
11. 工具链（fmt/lint/test/bench）
12. 测试体系（Test262、npm canary）

### Oracle 架构建议

1. **编译器分层**：SWC AST → Lowered IR → WASM（AOT）/ 机器码（JIT）
2. **内存模型**：自管 JS heap + linear memory，non-moving mark-sweep GC
3. **对象表示**：64-bit tagged value，NaN boxing 仅用于 number fast path
4. **模块系统**：ESM-first，CJS 通过兼容层适配
5. **Node API**：完整兼容，通过 host ABI 桥接
6. **多运行时**：先深度打磨 wasmtime，抽象 host ABI
7. **测试体系**：分层测试 → differential → Test262

### Metis 差距分析

**10 个关键未回答问题**（将在计划中明确）：
1. "100% npm 兼容"操作定义（Node-API、node-gyp、postinstall 等）
2. 兼容性单一真相源（Node LTS/Deno/Test262/包通过率）
3. 是否允许嵌入现有 JS 引擎处理极端动态特性
4. Native addon 策略
5. TS 类型检查完整度（tsc 等价性级别）
6. 首批强制宿主 API 清单
7. 生产就绪平台矩阵
8. 性能契约优先级
9. 最低安全边界
10. 阶段完成度量标准

**9 个重大风险**：
- Moonshot 堆叠风险（同时构建引擎+类型检查器+包管理器）
- 虚假进展风险（演示脚本不等于成熟度）
- 架构锁定风险（直接 AST→后端会导致重写债务）
- 兼容性矛盾风险（"精选 API" vs "100% npm 兼容"）
- 动态特性风险（eval/Proxy 可能破坏 AOT 假设）
- Native addon 风险（大量 npm 包依赖原生模块）
- 安全延期风险（后期安全硬化可能需重写基础）
- 测试债务风险（无 Test262 覆盖会导致实现超前于信心）
- 维护风险（需永久跟踪 Node/npm/TS/web API 演进）

**11 个关键路径依赖**（必须按顺序）：
1. 测试基础设施
2. 编译器前端分离（parse/semantic/backend）
3. 语义模型（scope/symbol/CFG）
4. 值模型 + 堆 + 对象布局
5. 执行模型（call frames/closures/exceptions/async）
6. 模块系统
7. CJS 互操作和包解析
8. 宿主 ABI 和内置 API
9. Native addon/FFI/Node-API
10. 混合 AOT+JIT 收敛
11. 性能和安全硬化

## Work Objectives

### 核心目标
构建与 Deno 同等成熟度的通用 JS/TS 运行时 wjsm，支持：
- 完整 JavaScript 语言特性（ES2024+）
- 完整 TypeScript 语言（解析 + 类型检查）
- AOT 编译到 WASM（静态代码）
- JIT 编译（动态代码：eval、new Function、动态 import）
- 100% npm 生态兼容（含 Node-API 原生扩展）
- 标准库覆盖（Node.js 内置模块 + Web Platform API）
- 开发工具链（fmt、lint、test、bench、LSP）
- 生产级性能和安全

### 交付物

1. **编译器基础设施**
   - 分层编译器架构（parser/semantic/lowering/backend）
   - Lowered IR 设计和实现
   - AOT WASM 后端
   - JIT 机器码后端

2. **运行时核心**
   - 值模型（tagged values、NaN boxing）
   - 堆分配器和 GC
   - 对象模型（对象、数组、函数、闭包）
   - 执行模型（call frames、exceptions、async/await）

3. **模块系统**
   - ESM 加载器和模块图
   - CJS 兼容层
   - npm 包解析和缓存
   - Import Maps
   - Lockfile

4. **TypeScript 支持**
   - 完整类型检查器（tsc 等价）
   - tsconfig.json 支持
   - 声明文件（.d.ts）处理
   - 增量检查

5. **标准库**
   - Node.js 内置模块全兼容
   - Web Platform API（fetch、WebCrypto、Worker）

6. **工具链**
   - CLI（run、build、fmt、lint、test、bench）
   - LSP 服务器
   - 包管理（install、cache）

7. **测试体系**
   - Test262 集成
   - npm 包 canary 套件
   - Node 行为 fixtures

### 完成定义

**阶段 1（基础设施）**：
- [ ] `cargo nextest run --color=never` 通过
- [ ] 编译器分层架构完成，IR 定义稳定
- [ ] 测试基础设施完整（unit/integration/fixture/snapshot）

**阶段 2（语义核心）**：
- [ ] 完整变量/作用域/函数/控制流支持
- [ ] Test262 基础测试套件通过 >80%
- [ ] 语义 fixtures 覆盖率 >90%

**阶段 3（对象和内存）**：
- [ ] 完整对象/数组/原型链支持
- [ ] GC 实现稳定，无内存泄漏
- [ ] 复杂对象操作 fixtures 通过

**阶段 4（模块和包）**：
- [ ] ESM/CJS 完整互操作
- [ ] npm 包安装和解析工作
- [ ] 1000+ 纯 JS npm 包 canary 通过

**阶段 5（JIT 和 TS）**：
- [ ] JIT 编译器工作（eval/new Function）
- [ ] TypeScript 类型检查器完整
- [ ] AOT+JIT 无缝切换

**阶段 6（API 和工具链）**：
- [ ] Node.js 内置模块覆盖率 >90%
- [ ] 完整 CLI 工具链（fmt/lint/test/bench）
- [ ] LSP 服务器功能完整

**阶段 7（生产就绪）**：
- [ ] Test262 完整测试套件通过率 >95%
- [ ] 10,000+ npm 包 canary 通过
- [ ] Node-API 兼容层工作
- [ ] 性能基准达到 Deno 80%
- [ ] 安全沙箱和权限模型

### Must Have

1. **编译器基础设施**
   - 分层架构（不可妥协）
   - Lowered IR（所有后端的基础）
   - 测试基础设施先行（TDD）

2. **语义核心**
   - 完整 JavaScript 语义（变量、作用域、闭包、this、原型链）
   - 异步模型（Promise、async/await、事件循环）

3. **内存和对象模型**
   - 自管堆和 GC
   - 统一的对象表示

4. **模块系统**
   - ESM-first（主路径）
   - CJS 兼容（npm 必需）

5. **AOT+JIT 混合**
   - 静态代码 AOT 到 WASM
   - 动态代码 JIT 到机器码
   - 统一运行时对象模型

6. **TypeScript 完整支持**
   - 解析 + 类型检查（不只是擦除）

7. **npm 完整兼容**
   - 包解析和安装
   - Node-API（原生扩展）

### Must NOT Have（防护栏）

1. **架构层面**
   - 不使用纯解释器路径（坚持 AOT+JIT 混合，不引入第三套执行路径）
   - 不直接扩展当前 AST→WASM 的 PoC 路径（必须先引入 IR）
   - 不在语义核心稳定前做复杂优化（inline cache、shape transition 等）

2. **兼容性层面**
   - 早期阶段不承诺 "100% npm 兼容"（除非已实现 Node-API）
   - 不做 Node.js 版本无限兼容（锁定特定 LTS 版本作为基准）

3. **测试层面**
   - 不接受仅演示脚本作为里程碑完成标准
   - 不延迟测试基础设施建设

4. **范围控制**
   - 每阶段必须有明确的 "Must NOT Have" 清单
   - 必须有明确的 descoping 触发条件

## Verification Strategy

### 测试决策

**策略**：TDD（RED-GREEN-REFACTOR）

**测试层级金字塔**：
1. **单元测试**：`cargo nextest run --color=never` — 所有 Rust 模块
2. **语义 fixtures**：JS 代码片段 → 预期行为验证
3. **IR 快照**：编译器 IR 输出快照对比
4. **WASM/后端快照**：生成的 WASM/机器码快照对比
5. **运行时集成**：端到端 JS 执行测试
6. **Test262**：ECMAScript 官方测试套件
7. **Node 行为 fixtures**：与 Node.js 行为对比
8. **npm canary**：真实 npm 包执行测试

**证据策略**：
- 每任务必须有可执行验收标准（具体命令 + 预期输出）
- 证据保存到 `.sisyphus/evidence/<phase>/<scenario>.{txt,diff,wasm}`
- 每任务必须有 happy path + failure path + regression 三种 QA 场景

### QA 场景模板

每个任务必须包含：

```
Scenario: [Happy path]
  Tool: Bash
  Steps:
    - cargo run -- run fixtures/happy/<case>.ts
  Expected: stdout matches fixtures/happy/<case>.expected
  Evidence: .sisyphus/evidence/<phase>/task-N-happy.txt

Scenario: [Failure path]
  Tool: Bash
  Steps:
    - cargo run -- run fixtures/errors/<case>.ts
  Expected: exit code != 0, stderr contains "<expected error>"
  Evidence: .sisyphus/evidence/<phase>/task-N-error.txt

Scenario: [Regression]
  Tool: Bash
  Steps:
    - cargo nextest run --color=never
  Expected: all tests pass
  Evidence: .sisyphus/evidence/<phase>/task-N-regression.txt
```

## Execution Strategy

### 阶段划分

项目分 7 大阶段，阶段间有依赖关系，阶段内高度并行：

**阶段 1：基础设施**（并行度：高）
- 测试基础设施
- 编译器分层架构
- IR 设计
- 项目结构重构

**阶段 2：语义核心**（并行度：高）
- 语义分析器
- 符号表
- 变量/作用域
- 控制流
- 函数和闭包

**阶段 3：运行时基础**（并行度：中等）
- 值模型
- 堆分配器
- 对象模型
- GC
- 执行模型（call frames、exceptions）

**阶段 4：异步和模块**（并行度：中等）
- Promise/microtask
- async/await
- ESM 加载器
- CJS 兼容
- 包解析

**阶段 5：JIT 和 TS**（并行度：高）
- JIT 编译器基础设施
- JIT 代码生成
- AOT+JIT 协调
- TS 类型检查器

**阶段 6：API 和工具链**（并行度：高）
- 宿主 ABI
- Node 内置模块
- Web Platform API
- CLI 工具链
- LSP 服务器

**阶段 7：生产就绪**（并行度：中等）
- 性能优化
- 安全沙箱
- Node-API 兼容
- 完整测试覆盖

### 依赖矩阵

```
阶段1 (基础设施)
  ├── 阶段2 (语义核心)
  │     ├── 阶段3 (运行时基础)
  │     │     ├── 阶段4 (异步和模块)
  │     │     │     ├── 阶段5 (JIT 和 TS)
  │     │     │     │     ├── 阶段6 (API 和工具链)
  │     │     │     │     │     └── 阶段7 (生产就绪)
```

### Agent 配置策略

**推荐 Agent Profile**：
- **Compiler tasks**：`unspecified-high` 或 `ultrabrain`（复杂算法）+ Rust 技能
- **Runtime tasks**：`unspecified-high`（内存管理、GC）+ Rust 技能
- **API/tasks**：`unspecified-high` + Node.js 技能
- **Tooling tasks**：`unspecified-high` + CLI/LSP 技能
- **Test tasks**：`quick`（测试编写、fixtures）

**Skill 需求**：
- `coding-guidelines`：所有 Rust 代码
- `next-best-practices`：如果有 Web/Next.js 相关
- `ast-grep`：代码分析和重构

## TODOs

> 以下为高阶任务分解，每个任务可在执行时进一步拆分。
> 所有任务必须满足：Agent Profile + 并行化信息 + 参考资料 + 验收标准 + QA 场景

### 阶段 1：基础设施

> 目标：建立可扩展的编译器架构和测试基础设施
> 预估：50+ 任务，2-3 个月

- [x] 1.1 设置 `cargo nextest` 测试基础设施

  **What to do**: 
  - 安装并配置 `cargo nextest` 作为项目测试运行器
  - 创建测试目录结构（unit、integration、fixture）
  - 配置 `.config/nextest.toml`
  - 迁移现有测试（如果有）到 nextest

  **Must NOT do**: 
  - 不使用 `cargo test`（违反项目规则）
  - 不在这个阶段编写业务测试，只建立基础设施

  **Recommended Agent Profile**:
  - Category: `quick`
  - Skills: [`coding-guidelines`]

  **Parallelization**: Can Parallel: NO | Wave: 1 | Blocks: [1.2, 1.3, 1.4] | Blocked By: []

  **References**:
  - 命令: `cargo nextest run --color=never`（所有后续任务的测试命令）

  **Acceptance Criteria**:
  - [ ] `cargo nextest run --color=never` 成功执行（即使无测试）
  - [ ] 测试目录结构存在

  **QA Scenarios**:
  ```
  Scenario: nextest 安装
    Tool: Bash
    Steps: cargo nextest --version
    Expected: 输出版本号
    Evidence: .sisyphus/evidence/phase1/task-1-1-nextest-version.txt

  Scenario: 空测试套件运行
    Tool: Bash
    Steps: cargo nextest run --color=never
    Expected: 成功（0个测试通过）
    Evidence: .sisyphus/evidence/phase1/task-1-1-empty-run.txt
  ```

  **Commit**: YES | Message: `chore: setup cargo nextest test infrastructure` | Files: [`.config/nextest.toml`, `tests/`]

- [x] 1.2 创建 fixtures 测试框架

  **What to do**:
  - 创建 fixtures 目录结构：`fixtures/{happy,errors,modules,semantic}/`
  - 实现 fixture runner：读取 `.js/.ts` 文件，执行，对比 `.expected` 文件
  - 支持 stdout/stderr/exit code 对比
  - 支持快照测试（首次生成，后续对比）

  **Must NOT do**:
  - 不实现复杂解析器，仅建立测试框架
  - 不涉及业务逻辑测试

  **Recommended Agent Profile**:
  - Category: `unspecified-high`
  - Skills: [`coding-guidelines`]

  **Parallelization**: Can Parallel: YES | Wave: 1 | Blocks: [1.3, 2.x] | Blocked By: [1.1]

  **References**:
  - Pattern: 参考 Deno/Bun 的 fixture 测试设计

  **Acceptance Criteria**:
  - [ ] Fixture runner 可执行
  - [ ] 示例 fixtures 可运行并对比输出

  **QA Scenarios**:
  ```
  Scenario: 简单 fixture 运行
    Tool: Bash
    Steps: cargo run -- run fixtures/happy/hello.js 2>&1
    Expected: 输出 "Hello"
    Evidence: .sisyphus/evidence/phase1/task-1-2-fixture-run.txt
  ```

  **Commit**: YES | Message: `test: add fixtures testing framework` | Files: [`tests/fixtures/`, `tests/fixture_runner.rs`]

- [ ] 1.3 重构项目目录结构

  **What to do**:
  - 创建 `crates/` 多 crate 工作区结构
  - 分离：parser、semantic、ir、backend_wasm、backend_jit、runtime、cli
  - 更新 `Cargo.toml` workspace 配置
  - 迁移现有代码到临时位置，保持编译通过

  **Must NOT do**:
  - 不改变现有代码逻辑，仅移动文件
  - 不拆分实现，仅建立目录结构

  **Recommended Agent Profile**:
  - Category: `unspecified-high`
  - Skills: [`coding-guidelines`]

  **Parallelization**: Can Parallel: YES | Wave: 1 | Blocks: [1.4] | Blocked By: [1.1]

  **References**:
  - 模式: `crates/wjsm-parser/`, `crates/wjsm-semantic/`, `crates/wjsm-ir/`, `crates/wjsm-backend-wasm/`, `crates/wjsm-backend-jit/`, `crates/wjsm-runtime/`, `crates/wjsm-cli/`

  **Acceptance Criteria**:
  - [ ] `cargo build` 成功
  - [ ] `cargo nextest run --color=never` 成功
  - [ ] 目录结构符合规划

  **QA Scenarios**:
  ```
  Scenario: 多 crate 构建
    Tool: Bash
    Steps: cargo build --workspace
    Expected: 成功编译所有 crates
    Evidence: .sisyphus/evidence/phase1/task-1-3-workspace-build.txt
  ```

  **Commit**: YES | Message: `refactor: restructure into workspace crates` | Files: [`Cargo.toml`, `crates/*/Cargo.toml`]

- [ ] 1.4 设计 Lowered IR

  **What to do**:
  - 定义中间表示（IR）数据结构
  - 设计：模块、函数、基本块、指令集
  - 支持：变量、控制流、函数调用、对象操作
  - 文档化 IR 设计决策

  **Must NOT do**:
  - 不实现 IR 生成，仅设计数据结构
  - 不涉及后端代码生成

  **Recommended Agent Profile**:
  - Category: `ultrabrain`
  - Skills: [`coding-guidelines`]

  **Parallelization**: Can Parallel: YES | Wave: 1 | Blocks: [1.5, 1.6, 2.x] | Blocked By: [1.3]

  **References**:
  - 参考: LLVM IR、Cranelift IR、Deno 的 swc 中间表示
  - 文档: `crates/wjsm-ir/docs/ir-design.md`

  **Acceptance Criteria**:
  - [ ] IR 数据结构定义在 `crates/wjsm-ir/src/`
  - [ ] 支持：Module、Function、BasicBlock、Instruction
  - [ ] 文档说明设计决策

  **QA Scenarios**:
  ```
  Scenario: IR 数据结构编译
    Tool: Bash
    Steps: cargo build -p wjsm-ir
    Expected: 成功编译
    Evidence: .sisyphus/evidence/phase1/task-1-4-ir-build.txt
  ```

  **Commit**: YES | Message: `feat(ir): design lowered IR data structures` | Files: [`crates/wjsm-ir/src/lib.rs`, `crates/wjsm-ir/docs/ir-design.md`]

- [ ] 1.5 实现 AST → IR  lowering（基础表达式）

  **What to do**:
  - 实现从 SWC AST 到 IR 的转换（lowering）
  - 支持：数字/字符串字面量、二元运算、console.log 调用
  - 保持与现有功能等价
  - 添加 IR 快照测试

  **Must NOT do**:
  - 不扩展新功能，仅重构现有能力到 IR
  - 不涉及新后端

  **Recommended Agent Profile**:
  - Category: `unspecified-high`
  - Skills: [`coding-guidelines`]

  **Parallelization**: Can Parallel: YES | Wave: 2 | Blocks: [1.6, 1.7] | Blocked By: [1.4]

  **References**:
  - 当前实现: `src/compiler/codegen.rs`
  - IR 定义: `crates/wjsm-ir/src/`

  **Acceptance Criteria**:
  - [ ] `console.log("Hello")` 生成正确 IR
  - [ ] `1 + 2 * 3` 生成正确 IR
  - [ ] IR 快照测试通过

  **QA Scenarios**:
  ```
  Scenario: 字面量 lowering
    Tool: Bash
    Steps: cargo nextest run --color=never -p wjsm-semantic lowering::literal
    Expected: 测试通过
    Evidence: .sisyphus/evidence/phase1/task-1-5-literal-lowering.txt

  Scenario: 二元运算 lowering
    Tool: Bash
    Steps: cargo nextest run --color=never -p wjsm-semantic lowering::binary
    Expected: 测试通过
    Evidence: .sisyphus/evidence/phase1/task-1-5-binary-lowering.txt
  ```

  **Commit**: YES | Message: `feat(semantic): implement basic AST to IR lowering` | Files: [`crates/wjsm-semantic/src/lowering.rs`]

- [ ] 1.6 实现 IR → WASM backend（基础表达式）

  **What to do**:
  - 实现 IR 到 WASM 的代码生成
  - 支持：数字/字符串字面量、二元运算、console.log 调用
  - 使用 `wasm-encoder`
  - 与现有 `codegen.rs` 功能等价

  **Must NOT do**:
  - 不扩展新功能
  - 不改变运行时接口

  **Recommended Agent Profile**:
  - Category: `unspecified-high`
  - Skills: [`coding-guidelines`]

  **Parallelization**: Can Parallel: YES | Wave: 2 | Blocks: [1.7] | Blocked By: [1.4]

  **References**:
  - 当前实现: `src/compiler/codegen.rs`
  - IR 定义: `crates/wjsm-ir/src/`

  **Acceptance Criteria**:
  - [ ] IR → WASM 代码生成工作
  - [ ] `console.log("Hello")` 生成可执行 WASM
  - [ ] WASM 快照测试通过

  **QA Scenarios**:
  ```
  Scenario: WASM 代码生成
    Tool: Bash
    Steps: cargo run -- build fixtures/happy/hello.ts -o /tmp/test.wasm && wasm-objdump -x /tmp/test.wasm
    Expected: 生成有效 WASM 模块
    Evidence: .sisyphus/evidence/phase1/task-1-6-wasm-gen.txt
  ```

  **Commit**: YES | Message: `feat(backend): implement IR to WASM backend for basic expressions` | Files: [`crates/wjsm-backend-wasm/src/codegen.rs`]

- [ ] 1.7 集成新架构到 CLI

  **What to do**:
  - 更新 CLI 使用新架构：`parse → lower → codegen`
  - 保持 CLI 接口不变（`build`/`run` 命令）
  - 确保端到端功能等价
  - 移除旧 `codegen.rs` 代码

  **Must NOT do**:
  - 不改变 CLI 接口
  - 不添加新功能

  **Recommended Agent Profile**:
  - Category: `unspecified-high`
  - Skills: [`coding-guidelines`]

  **Parallelization**: Can Parallel: NO | Wave: 3 | Blocks: [阶段2] | Blocked By: [1.5, 1.6]

  **References**:
  - CLI: `crates/wjsm-cli/src/main.rs`
  - 原实现: `src/main.rs`

  **Acceptance Criteria**:
  - [ ] `cargo run -- run test.ts` 输出 "Hello World"
  - [ ] `cargo run -- build test.ts -o out.wasm` 生成可执行 WASM
  - [ ] 功能与重构前等价

  **QA Scenarios**:
  ```
  Scenario: 端到端功能测试
    Tool: Bash
    Steps: |
      echo 'console.log("Hello World"); console.log(1 + 2 * 3);' > /tmp/test.ts
      cargo run -- run /tmp/test.ts
    Expected: |
      Hello World
      7
    Evidence: .sisyphus/evidence/phase1/task-1-7-e2e.txt
  ```

  **Commit**: YES | Message: `refactor(cli): integrate new compiler architecture` | Files: [`crates/wjsm-cli/src/main.rs`, 删除 `src/compiler/codegen.rs`]

### 阶段 2：语义核心

> 目标：实现完整 JavaScript 语义（变量、作用域、函数、控制流）
> 预估：100+ 任务，4-6 个月
> 依赖：阶段 1 完成

- [ ] 2.1 实现符号表（Symbol Table）

  **What to do**:
  - 实现符号表数据结构：标识符 → 符号信息
  - 支持：变量、函数、类、导入/导出符号
  - 支持作用域层级（block、function、module、global）
  - 实现符号解析逻辑

  **Must NOT do**:
  - 不实现类型检查（那是阶段 5）
  - 不实现变量提升的复杂边界情况

  **Recommended Agent Profile**:
  - Category: `unspecified-high`
  - Skills: [`coding-guidelines`]

  **Parallelization**: Can Parallel: YES | Wave: 1 | Blocks: [2.2, 2.3, 2.4] | Blocked By: [1.7]

  **References**:
  - 理论: 编译原理符号表、ECMAScript 作用域规则
  - 参考: swc 的 scope analysis

  **Acceptance Criteria**:
  - [ ] 符号表可编译
  - [ ] 简单变量声明可被记录和解析
  - [ ] 单元测试通过

  **QA Scenarios**:
  ```
  Scenario: 符号表基础
    Tool: Bash
    Steps: cargo nextest run --color=never -p wjsm-semantic symbol_table
    Expected: 测试通过
    Evidence: .sisyphus/evidence/phase2/task-2-1-symbol-table.txt
  ```

  **Commit**: YES | Message: `feat(semantic): implement symbol table` | Files: [`crates/wjsm-semantic/src/symbol_table.rs`]

- [ ] 2.2 实现作用域分析（Scope Analysis）

  **What to do**:
  - 实现作用域树构建
  - 支持：全局作用域、函数作用域、块级作用域（let/const）
  - 实现标识符解析（在当前作用域和父作用域查找）
  - 处理 TDZ（Temporal Dead Zone）基础情况

  **Must NOT do**:
  - 不处理所有 TDZ 边界情况
  - 不实现 with/eval 的动态作用域

  **Recommended Agent Profile**:
  - Category: `unspecified-high`
  - Skills: [`coding-guidelines`]

  **Parallelization**: Can Parallel: YES | Wave: 1 | Blocks: [2.3, 2.4] | Blocked By: [2.1]

  **References**:
  - 规范: ECMAScript 规范 8.1 Lexical Environments
  - 参考: swc scope analysis

  **Acceptance Criteria**:
  - [ ] 作用域分析通过测试
  - [ ] 嵌套作用域变量解析正确

  **QA Scenarios**:
  ```
  Scenario: 嵌套作用域
    Tool: Bash
    Steps: cargo nextest run --color=never -p wjsm-semantic scope
    Expected: 测试通过
    Evidence: .sisyphus/evidence/phase2/task-2-2-scope.txt
  ```

  **Commit**: YES | Message: `feat(semantic): implement scope analysis` | Files: [`crates/wjsm-semantic/src/scope.rs`]

- [ ] 2.3 实现变量声明 lowering（let/const/var）

  **What to do**:
  - 扩展 lowering 支持变量声明
  - IR 添加：局部变量分配、存储、加载指令
  - 支持：let、const、var
  - 后端实现变量存储（WASM local 或线性内存）

  **Must NOT do**:
  - 不实现变量提升（hoisting）的复杂情况
  - 不实现全局对象属性创建（var 在全局作用域）

  **Recommended Agent Profile**:
  - Category: `unspecified-high`
  - Skills: [`coding-guidelines`]

  **Parallelization**: Can Parallel: YES | Wave: 2 | Blocks: [2.4, 2.5] | Blocked By: [2.2]

  **References**:
  - 当前: `crates/wjsm-semantic/src/lowering.rs`
  - IR: `crates/wjsm-ir/src/`

  **Acceptance Criteria**:
  - [ ] `let x = 1; console.log(x);` 工作
  - [ ] `const y = 2;` 工作
  - [ ] fixtures 测试通过

  **QA Scenarios**:
  ```
  Scenario: 变量声明
    Tool: Bash
    Steps: cargo run -- run fixtures/happy/variable_decl.js
    Expected: 输出正确值
    Evidence: .sisyphus/evidence/phase2/task-2-3-variable.txt
  ```

  **Commit**: YES | Message: `feat: implement variable declaration lowering` | Files: [`crates/wjsm-semantic/src/lowering.rs`, `crates/wjsm-ir/src/instructions.rs`]

- [ ] 2.4 实现控制流 lowering（if/else）

  **What to do**:
  - 扩展 IR 支持条件分支
  - 实现 if/else AST → IR lowering
  - 实现 truthiness 检查（JS 语义）
  - 后端生成 WASM br/br_if

  **Must NOT do**:
  - 不实现短路求优（&&/||）
  - 不实现三元运算符

  **Recommended Agent Profile**:
  - Category: `unspecified-high`
  - Skills: [`coding-guidelines`]

  **Parallelization**: Can Parallel: YES | Wave: 2 | Blocks: [2.5] | Blocked By: [2.2]

  **References**:
  - JS 语义: ECMAScript 13.6 If Statement

  **Acceptance Criteria**:
  - [ ] `if (true) { console.log(1); } else { console.log(2); }` 工作
  - [ ] truthiness 符合 JS 规范

  **QA Scenarios**:
  ```
  Scenario: if/else 控制流
    Tool: Bash
    Steps: cargo run -- run fixtures/happy/if_else.js
    Expected: 输出 "1"
    Evidence: .sisyphus/evidence/phase2/task-2-4-ifelse.txt
  ```

  **Commit**: YES | Message: `feat: implement if/else control flow` | Files: [`crates/wjsm-semantic/src/lowering.rs`, `crates/wjsm-ir/src/control_flow.rs`]

- [ ] 2.5 实现循环 lowering（while/for）

  **What to do**:
  - 实现 while/for 循环 lowering
  - IR 支持循环结构（或展开为跳转）
  - 支持 break/continue
  - 支持 for-in/for-of（基础情况）

  **Must NOT do**:
  - 不实现 for-await-of
  - 不处理复杂迭代器协议

  **Recommended Agent Profile**:
  - Category: `unspecified-high`
  - Skills: [`coding-guidelines`]

  **Parallelization**: Can Parallel: YES | Wave: 3 | Blocks: [2.6] | Blocked By: [2.4]

  **References**:
  - JS 语义: ECMAScript 13.7 Iteration Statements

  **Acceptance Criteria**:
  - [ ] `while` 循环工作
  - [ ] `for` 循环工作
  - [ ] `break`/`continue` 工作

  **QA Scenarios**:
  ```
  Scenario: while 循环
    Tool: Bash
    Steps: cargo run -- run fixtures/happy/while_loop.js
    Expected: 输出循环结果
    Evidence: .sisyphus/evidence/phase2/task-2-5-while.txt
  ```

  **Commit**: YES | Message: `feat: implement while/for loops` | Files: [`crates/wjsm-semantic/src/lowering.rs`]

- [ ] 2.6 实现函数声明和调用 lowering

  **What to do**:
  - 实现函数声明 lowering
  - IR 支持函数定义、调用、返回
  - 支持参数传递
  - 后端生成 WASM function/import/call
  - 支持递归调用

  **Must NOT do**:
  - 不实现闭包（阶段 3）
  - 不实现 this 绑定（阶段 3）
  - 不实现默认参数/剩余参数/展开

  **Recommended Agent Profile**:
  - Category: `unspecified-high`
  - Skills: [`coding-guidelines`]

  **Parallelization**: Can Parallel: YES | Wave: 3 | Blocks: [2.7, 3.1] | Blocked By: [2.5]

  **References**:
  - JS 语义: ECMAScript 14 Functions and Classes
  - WASM: function types, call_indirect

  **Acceptance Criteria**:
  - [ ] `function add(a, b) { return a + b; }` 工作
  - [ ] 递归函数工作
  - [ ] fixtures 测试通过

  **QA Scenarios**:
  ```
  Scenario: 函数调用
    Tool: Bash
    Steps: cargo run -- run fixtures/happy/function_call.js
    Expected: 函数返回值正确
    Evidence: .sisyphus/evidence/phase2/task-2-6-function.txt
  ```

  **Commit**: YES | Message: `feat: implement function declaration and call` | Files: [`crates/wjsm-semantic/src/lowering.rs`, `crates/wjsm-ir/src/functions.rs`]

- [ ] 2.7 实现异常处理 lowering（try/catch/throw）

  **What to do**:
  - 实现 try/catch/throw/finally lowering
  - IR 支持异常相关指令
  - 后端利用 WASM exception handling proposal 或 setjmp/longjmp 模拟
  - 运行时支持异常传播

  **Must NOT do**:
  - 不实现 Error 子类完整层次
  - 不实现 stack trace 捕获

  **Recommended Agent Profile**:
  - Category: `ultrabrain`
  - Skills: [`coding-guidelines`]

  **Parallelization**: Can Parallel: YES | Wave: 4 | Blocks: [2.8, 3.1] | Blocked By: [2.6]

  **References**:
  - JS 语义: ECMAScript 14.15 Try Statement
  - WASM: Exception handling proposal

  **Acceptance Criteria**:
  - [ ] `try { throw "err"; } catch(e) { }` 工作
  - [ ] finally 块执行

  **QA Scenarios**:
  ```
  Scenario: 异常捕获
    Tool: Bash
    Steps: cargo run -- run fixtures/happy/try_catch.js
    Expected: 捕获异常并输出
    Evidence: .sisyphus/evidence/phase2/task-2-7-exception.txt
  ```

  **Commit**: YES | Message: `feat: implement try/catch/throw` | Files: [`crates/wjsm-semantic/src/lowering.rs`, `crates/wjsm-backend-wasm/src/exception.rs`]

- [ ] 2.8 实现标识符表达式和赋值

  **What to do**:
  - 实现标识符读取 lowering（变量查找）
  - 实现赋值表达式 lowering（=、+=、-= 等）
  - 支持递增/递减（++/--）
  - 支持复合赋值

  **Must NOT do**:
  - 不实现解构赋值（阶段 4）

  **Recommended Agent Profile**:
  - Category: `unspecified-high`
  - Skills: [`coding-guidelines`]

  **Parallelization**: Can Parallel: YES | Wave: 4 | Blocks: [阶段3] | Blocked By: [2.6]

  **References**:
  - 当前 lowering 实现
  - 符号表和作用域

  **Acceptance Criteria**:
  - [ ] `let x = 1; x = 2;` 工作
  - [ ] `x += 1;` 工作
  - [ ] `x++;` 工作

  **QA Scenarios**:
  ```
  Scenario: 赋值操作
    Tool: Bash
    Steps: cargo run -- run fixtures/happy/assignment.js
    Expected: 赋值后值正确
    Evidence: .sisyphus/evidence/phase2/task-2-8-assignment.txt
  ```

  **Commit**: YES | Message: `feat: implement identifier expressions and assignment` | Files: [`crates/wjsm-semantic/src/lowering.rs`]

### 阶段 3：运行时基础

> 目标：实现值模型、堆分配、对象模型、GC、执行模型
> 预估：80+ 任务，3-4 个月
> 依赖：阶段 2 完成

- [ ] 3.1 实现 Tagged Value 系统

  **What to do**:
  - 定义 64-bit tagged value 表示
  - 支持：undefined、null、boolean、number（NaN boxing）、string pointer、object pointer、function pointer
  - 实现类型检查和转换函数
  - 实现值操作（add、compare 等）符合 JS 语义

  **Must NOT do**:
  - 不实现大整数（BigInt）
  - 不实现 Symbol

  **Recommended Agent Profile**:
  - Category: `ultrabrain`
  - Skills: [`coding-guidelines`]

  **Parallelization**: Can Parallel: YES | Wave: 1 | Blocks: [3.2, 3.3, 3.4, 3.5] | Blocked By: [阶段2]

  **References**:
  - 当前: `src/compiler/value.rs`（仅 NaN boxing）
  - 理论: V8 的 tagged pointer、SpiderMonkey 的 Value 表示
  - 文档: `crates/wjsm-runtime/docs/value-model.md`

  **Acceptance Criteria**:
  - [ ] Tagged value 定义完整
  - [ ] 类型转换测试通过
  - [ ] JS 类型判断（typeof）正确

  **QA Scenarios**:
  ```
  Scenario: tagged value 基础
    Tool: Bash
    Steps: cargo nextest run --color=never -p wjsm-runtime value
    Expected: 测试通过
    Evidence: .sisyphus/evidence/phase3/task-3-1-tagged-value.txt
  ```

  **Commit**: YES | Message: `feat(runtime): implement tagged value system` | Files: [`crates/wjsm-runtime/src/value.rs`]

- [ ] 3.2 实现堆分配器

  **What to do**:
  - 实现 WASM 线性内存中的堆分配器
  - 支持：malloc/free 语义
  - 实现 bump allocator 或 segregated allocator
  - 支持内存对齐
  - 集成到运行时

  **Must NOT do**:
  - 不实现 GC（阶段 3.4）
  - 不实现 compaction

  **Recommended Agent Profile**:
  - Category: `unspecified-high`
  - Skills: [`coding-guidelines`]

  **Parallelization**: Can Parallel: YES | Wave: 1 | Blocks: [3.3] | Blocked By: [3.1]

  **References**:
  - 理论: dlmalloc、jemalloc、V8 的 heap
  - WASM: 线性内存管理

  **Acceptance Criteria**:
  - [ ] 分配器可分配和释放内存
  - [ ] 无内存泄漏（简单测试）
  - [ ] 分配对齐正确

  **QA Scenarios**:
  ```
  Scenario: 堆分配
    Tool: Bash
    Steps: cargo nextest run --color=never -p wjsm-runtime allocator
    Expected: 测试通过
    Evidence: .sisyphus/evidence/phase3/task-3-2-allocator.txt
  ```

  **Commit**: YES | Message: `feat(runtime): implement heap allocator` | Files: [`crates/wjsm-runtime/src/allocator.rs`]

- [ ] 3.3 实现对象模型（基础）

  **What to do**:
  - 设计对象内存布局：header + properties
  - 实现对象创建函数
  - 实现属性存储和读取（简单情况）
  - 支持字符串和数字字面量作为属性键
  - 运行时提供宿主函数支持对象操作

  **Must NOT do**:
  - 不实现原型链（阶段 3.5）
  - 不实现 getter/setter
  - 不实现 Symbol 属性键

  **Recommended Agent Profile**:
  - Category: `ultrabrain`
  - Skills: [`coding-guidelines`]

  **Parallelization**: Can Parallel: YES | Wave: 2 | Blocks: [3.4, 3.5] | Blocked By: [3.2]

  **References**:
  - 理论: V8 的 JSObject、Hidden Classes（Shapes）
  - JS 语义: ECMAScript 6.1.7.2 Object

  **Acceptance Criteria**:
  - [ ] `let obj = { a: 1 };` 工作
  - [ ] `obj.a` 读取正确
  - [ ] `obj.b = 2` 赋值正确

  **QA Scenarios**:
  ```
  Scenario: 对象创建和访问
    Tool: Bash
    Steps: cargo run -- run fixtures/happy/object_basic.js
    Expected: 对象属性正确
    Evidence: .sisyphus/evidence/phase3/task-3-3-object-basic.txt
  ```

  **Commit**: YES | Message: `feat(runtime): implement basic object model` | Files: [`crates/wjsm-runtime/src/object.rs`, `crates/wjsm-runtime/src/heap.rs`]

- [ ] 3.4 实现标记-清除 GC

  **What to do**:
  - 实现非移动式标记-清除 GC
  - 实现 root 枚举（栈扫描、全局变量）
  - 实现 mark phase（遍历对象图）
  - 实现 sweep phase（回收未标记对象）
  - 集成到分配器

  **Must NOT do**:
  - 不实现分代 GC
  - 不实现增量 GC
  - 不实现 compaction

  **Recommended Agent Profile**:
  - Category: `ultrabrain`
  - Skills: [`coding-guidelines`]

  **Parallelization**: Can Parallel: YES | Wave: 3 | Blocks: [3.5, 3.6] | Blocked By: [3.3]

  **References**:
  - 理论: 垃圾回收算法（Jones & Lins）
  - 实现: V8 的 GC、Rust 的 gc 库

  **Acceptance Criteria**:
  - [ ] GC 可正确回收垃圾
  - [ ] 不会误回收存活对象
  - [ ] 循环引用可被回收

  **QA Scenarios**:
  ```
  Scenario: GC 基本功能
    Tool: Bash
    Steps: cargo nextest run --color=never -p wjsm-runtime gc
    Expected: 测试通过
    Evidence: .sisyphus/evidence/phase3/task-3-4-gc.txt
  ```

  **Commit**: YES | Message: `feat(runtime): implement mark-sweep GC` | Files: [`crates/wjsm-runtime/src/gc.rs`]

- [ ] 3.5 实现原型链

  **What to do**:
  - 在对象 header 中添加 prototype 指针
  - 实现属性查找（原型链遍历）
  - 实现 Object.create
  - 实现 Object.getPrototypeOf
  - 处理原型链循环检测

  **Must NOT do**:
  - 不实现 Object.setPrototypeOf（改变原型，复杂）
  - 不实现 __proto__

  **Recommended Agent Profile**:
  - Category: `unspecified-high`
  - Skills: [`coding-guidelines`]

  **Parallelization**: Can Parallel: YES | Wave: 4 | Blocks: [3.6, 3.7] | Blocked By: [3.4]

  **References**:
  - JS 语义: ECMAScript 9.1 Ordinary Object Internal Methods

  **Acceptance Criteria**:
  - [ ] 原型链属性查找工作
  - [ ] `Object.create` 工作

  **QA Scenarios**:
  ```
  Scenario: 原型链
    Tool: Bash
    Steps: cargo run -- run fixtures/happy/prototype_chain.js
    Expected: 原型属性正确继承
    Evidence: .sisyphus/evidence/phase3/task-3-5-prototype.txt
  ```

  **Commit**: YES | Message: `feat(runtime): implement prototype chain` | Files: [`crates/wjsm-runtime/src/prototype.rs`]

- [ ] 3.6 实现闭包

  **What to do**:
  - 实现函数环境记录（Environment Record）
  - 实现闭包对象（函数 + 捕获的环境）
  - 实现变量捕获（upvalue）
  - 支持嵌套函数访问外部变量
  - 正确管理闭包生命周期（GC 交互）

  **Must NOT do**:
  - 不实现箭头函数（阶段 4）
  - 不实现 this 词法绑定优化

  **Recommended Agent Profile**:
  - Category: `ultrabrain`
  - Skills: [`coding-guidelines`]

  **Parallelization**: Can Parallel: YES | Wave: 4 | Blocks: [3.7, 4.1] | Blocked By: [3.5]

  **References**:
  - JS 语义: ECMAScript 8.1 Lexical Environments
  - 实现: Lua 的 upvalue、V8 的 context chain

  **Acceptance Criteria**:
  - [ ] 闭包可捕获外部变量
  - [ ] 嵌套闭包工作
  - [ ] 闭包生命周期管理正确

  **QA Scenarios**:
  ```
  Scenario: 闭包
    Tool: Bash
    Steps: cargo run -- run fixtures/happy/closure.js
    Expected: 闭包正确捕获和访问变量
    Evidence: .sisyphus/evidence/phase3/task-3-6-closure.txt
  ```

  **Commit**: YES | Message: `feat(runtime): implement closures` | Files: [`crates/wjsm-runtime/src/closure.rs`, `crates/wjsm-runtime/src/environment.rs`]

- [ ] 3.7 实现数组

  **What to do**:
  - 设计数组内存布局（连续存储 + 长度）
  - 实现数组字面量 lowering
  - 实现数组索引访问和赋值
  - 实现数组长度属性
  - 实现 push/pop 等基本方法

  **Must NOT do**:
  - 不实现稀疏数组优化
  - 不实现所有数组方法（阶段 6）

  **Recommended Agent Profile**:
  - Category: `unspecified-high`
  - Skills: [`coding-guidelines`]

  **Parallelization**: Can Parallel: YES | Wave: 5 | Blocks: [阶段4] | Blocked By: [3.6]

  **References**:
  - JS 语义: ECMAScript 23.1 Array Objects

  **Acceptance Criteria**:
  - [ ] `[1, 2, 3]` 数组字面量工作
  - [ ] `arr[0]` 索引访问工作
  - [ ] `arr.push(4)` 工作

  **QA Scenarios**:
  ```
  Scenario: 数组操作
    Tool: Bash
    Steps: cargo run -- run fixtures/happy/array_basic.js
    Expected: 数组操作正确
    Evidence: .sisyphus/evidence/phase3/task-3-7-array.txt
  ```

  **Commit**: YES | Message: `feat(runtime): implement arrays` | Files: [`crates/wjsm-runtime/src/array.rs`]

- [ ] 3.8 实现执行模型（call frames、this 绑定）

  **What to do**:
  - 实现调用栈帧（call frames）
  - 实现 this 绑定（全局调用、方法调用、构造函数）
  - 实现函数调用约定
  - 支持 apply/call 基础（阶段 6）

  **Must NOT do**:
  - 不实现 new.target
  - 不实现 super

  **Recommended Agent Profile**:
  - Category: `unspecified-high`
  - Skills: [`coding-guidelines`]

  **Parallelization**: Can Parallel: YES | Wave: 5 | Blocks: [4.1] | Blocked By: [3.6]

  **References**:
  - JS 语义: ECMAScript 9.2 ECMAScript Function Objects

  **Acceptance Criteria**:
  - [ ] this 绑定正确
  - [ ] 方法调用中 this 指向对象

  **QA Scenarios**:
  ```
  Scenario: this 绑定
    Tool: Bash
    Steps: cargo run -- run fixtures/happy/this_binding.js
    Expected: this 值正确
    Evidence: .sisyphus/evidence/phase3/task-3-8-this.txt
  ```

  **Commit**: YES | Message: `feat(runtime): implement call frames and this binding` | Files: [`crates/wjsm-runtime/src/call_stack.rs`, `crates/wjsm-runtime/src/this.rs`]

### 阶段 4：异步和模块系统

> 目标：实现 Promise、async/await、ESM/CJS 模块系统、npm 包解析
> 预估：100+ 任务，4-6 个月
> 依赖：阶段 3 完成

- [ ] 4.1 实现事件循环基础（Event Loop）

  **What to do**:
  - 设计事件循环架构（macrotask + microtask 队列）
  - 实现任务队列数据结构
  - 实现事件循环驱动程序
  - 支持同步代码执行和任务调度

  **Must NOT do**:
  - 不实现定时器（阶段 4.2）
  - 不实现 I/O 事件（阶段 6）

  **Recommended Agent Profile**:
  - Category: `unspecified-high`
  - Skills: [`coding-guidelines`]

  **Parallelization**: Can Parallel: YES | Wave: 1 | Blocks: [4.2, 4.3] | Blocked By: [阶段3]

  **References**:
  - 理论: Node.js 事件循环、HTML 规范 Event Loops

  **Acceptance Criteria**:
  - [ ] 事件循环架构设计文档
  - [ ] 任务队列可工作

  **QA Scenarios**:
  ```
  Scenario: 事件循环基础
    Tool: Bash
    Steps: cargo nextest run --color=never -p wjsm-runtime event_loop
    Expected: 测试通过
    Evidence: .sisyphus/evidence/phase4/task-4-1-event-loop.txt
  ```

  **Commit**: YES | Message: `feat(runtime): implement event loop foundation` | Files: [`crates/wjsm-runtime/src/event_loop.rs`]

- [ ] 4.2 实现定时器（setTimeout/setInterval）

  **What to do**:
  - 实现 setTimeout/setInterval 宿主函数
  - 集成到事件循环
  - 实现 clearTimeout/clearInterval
  - 宿主 ABI 提供时间功能

  **Must NOT do**:
  - 不实现高精度的 setImmediate

  **Recommended Agent Profile**:
  - Category: `unspecified-high`
  - Skills: [`coding-guidelines`]

  **Parallelization**: Can Parallel: YES | Wave: 2 | Blocks: [4.3] | Blocked By: [4.1]

  **References**:
  - Node.js: timers 模块

  **Acceptance Criteria**:
  - [ ] `setTimeout(() => console.log("hi"), 100)` 工作
  - [ ] 定时器可被清除

  **QA Scenarios**:
  ```
  Scenario: 定时器
    Tool: Bash
    Steps: cargo run -- run fixtures/happy/timer.js
    Expected: 定时回调执行
    Evidence: .sisyphus/evidence/phase4/task-4-2-timer.txt
  ```

  **Commit**: YES | Message: `feat(runtime): implement timers` | Files: [`crates/wjsm-runtime/src/timers.rs`]

- [ ] 4.3 实现 Promise

  **What to do**:
  - 实现 Promise 对象和状态机
  - 实现 then/catch/finally
  - 实现 Promise.resolve/Promise.reject
  - 实现微任务队列（microtask）
  - 集成到事件循环

  **Must NOT do**:
  - 不实现 Promise.all/race/any/allSettled（阶段 4.4）

  **Recommended Agent Profile**:
  - Category: `ultrabrain`
  - Skills: [`coding-guidelines`]

  **Parallelization**: Can Parallel: YES | Wave: 2 | Blocks: [4.4, 4.5] | Blocked By: [4.2]

  **References**:
  - JS 语义: ECMAScript 27.2 Promise Objects
  - 实现: 参考 JS 引擎 Promise 实现

  **Acceptance Criteria**:
  - [ ] `new Promise((resolve) => resolve(1))` 工作
  - [ ] `.then()` 链式调用工作
  - [ ] 微任务在宏任务前执行

  **QA Scenarios**:
  ```
  Scenario: Promise 基本
    Tool: Bash
    Steps: cargo run -- run fixtures/happy/promise_basic.js
    Expected: Promise 状态正确
    Evidence: .sisyphus/evidence/phase4/task-4-3-promise.txt
  ```

  **Commit**: YES | Message: `feat(runtime): implement Promise` | Files: [`crates/wjsm-runtime/src/promise.rs`]

- [ ] 4.4 实现 async/await

  **What to do**:
  - 将 async 函数 lowering 为 Promise + generator 模式
  - 实现 await 语法糖转换
  - IR 支持异步控制流
  - 实现 async 函数调用约定

  **Must NOT do**:
  - 不实现 generator 函数作为独立功能
  - 不实现 for-await-of

  **Recommended Agent Profile**:
  - Category: `ultrabrain`
  - Skills: [`coding-guidelines`]

  **Parallelization**: Can Parallel: YES | Wave: 3 | Blocks: [4.5] | Blocked By: [4.3]

  **References**:
  - JS 语义: ECMAScript 27.8 Async Function Objects

  **Acceptance Criteria**:
  - [ ] `async function foo() { return 1; }` 工作
  - [ ] `await` 正确等待 Promise

  **QA Scenarios**:
  ```
  Scenario: async/await
    Tool: Bash
    Steps: cargo run -- run fixtures/happy/async_await.js
    Expected: 异步函数正确执行
    Evidence: .sisyphus/evidence/phase4/task-4-4-async.txt
  ```

  **Commit**: YES | Message: `feat: implement async/await` | Files: [`crates/wjsm-semantic/src/lowering.rs`, `crates/wjsm-ir/src/async.rs`]

- [ ] 4.5 实现 ESM 加载器基础

  **What to do**:
  - 实现模块解析（URL/文件路径 → 模块标识符）
  - 实现模块图构建（依赖分析）
  - 支持 import/export 语法 lowering
  - 实现模块实例化（绑定创建）
  - 实现模块执行（按拓扑顺序）

  **Must NOT do**:
  - 不实现动态 import（阶段 4.6）
  - 不实现循环依赖的复杂情况

  **Recommended Agent Profile**:
  - Category: `ultrabrain`
  - Skills: [`coding-guidelines`]

  **Parallelization**: Can Parallel: YES | Wave: 3 | Blocks: [4.6, 4.7, 5.1] | Blocked By: [4.4]

  **References**:
  - JS 语义: ECMAScript 16 Modules
  - 实现: Deno/Node.js 的模块加载器

  **Acceptance Criteria**:
  - [ ] `import { foo } from "./mod.js"` 工作
  - [ ] `export const bar = 1` 工作

  **QA Scenarios**:
  ```
  Scenario: ESM 基础
    Tool: Bash
    Steps: cargo run -- run fixtures/modules/esm_basic/main.js
    Expected: 模块导入导出正确
    Evidence: .sisyphus/evidence/phase4/task-4-5-esm.txt
  ```

  **Commit**: YES | Message: `feat: implement ESM loader foundation` | Files: [`crates/wjsm-loader/src/esm.rs`]

- [ ] 4.6 实现动态 import()

  **What to do**:
  - 实现 `import()` 表达式 lowering
  - 运行时支持动态模块加载
  - 动态模块解析和实例化
  - 返回 Promise<module>

  **Must NOT do**:
  - 不实现顶层 await（阶段 4.7）

  **Recommended Agent Profile**:
  - Category: `unspecified-high`
  - Skills: [`coding-guidelines`]

  **Parallelization**: Can Parallel: YES | Wave: 4 | Blocks: [4.7] | Blocked By: [4.5]

  **References**:
  - JS 语义: ECMAScript 16.2.2 Import Calls

  **Acceptance Criteria**:
  - [ ] `import("./mod.js")` 返回 Promise
  - [ ] 动态模块执行正确

  **QA Scenarios**:
  ```
  Scenario: 动态导入
    Tool: Bash
    Steps: cargo run -- run fixtures/modules/dynamic_import/main.js
    Expected: 动态模块加载成功
    Evidence: .sisyphus/evidence/phase4/task-4-6-dynamic-import.txt
  ```

  **Commit**: YES | Message: `feat: implement dynamic import()` | Files: [`crates/wjsm-loader/src/dynamic_import.rs`]

- [ ] 4.7 实现 CJS 兼容层

  **What to do**:
  - 实现 require() 函数
  - 实现 module.exports/exports
  - 实现 __filename/__dirname
  - 实现 CJS 模块包装器
  - ESM 和 CJS 互操作（interop）

  **Must NOT do**:
  - 不实现所有 Node.js 模块语义边界情况

  **Recommended Agent Profile**:
  - Category: `ultrabrain`
  - Skills: [`coding-guidelines`]

  **Parallelization**: Can Parallel: YES | Wave: 4 | Blocks: [4.8] | Blocked By: [4.5]

  **References**:
  - Node.js: CommonJS 模块系统

  **Acceptance Criteria**:
  - [ ] `require("./mod.js")` 工作
  - [ ] `module.exports = {}` 工作

  **QA Scenarios**:
  ```
  Scenario: CJS 基础
    Tool: Bash
    Steps: cargo run -- run fixtures/modules/cjs_basic/main.js
    Expected: CJS 模块工作
    Evidence: .sisyphus/evidence/phase4/task-4-7-cjs.txt
  ```

  **Commit**: YES | Message: `feat: implement CommonJS compatibility layer` | Files: [`crates/wjsm-loader/src/cjs.rs`]

- [ ] 4.8 实现 npm 包解析

  **What to do**:
  - 实现 package.json 读取和解析
  - 实现 "main"/"module"/"exports" 字段解析
  - 实现 node_modules 目录查找算法
  - 实现 npm: 导入前缀支持
  - 缓存已下载包

  **Must NOT do**:
  - 不实现包安装（npm install 等价物）
  - 不实现 postinstall 脚本

  **Recommended Agent Profile**:
  - Category: `unspecified-high`
  - Skills: [`coding-guidelines`]

  **Parallelization**: Can Parallel: YES | Wave: 5 | Blocks: [阶段5, 6.1] | Blocked By: [4.7]

  **References**:
  - Node.js: 包解析算法
  - npm: package.json 规范

  **Acceptance Criteria**:
  - [ ] `import foo from "npm:lodash"` 工作
  - [ ] node_modules 包可被解析

  **QA Scenarios**:
  ```
  Scenario: npm 包导入
    Tool: Bash
    Steps: cargo run -- run fixtures/npm/npm_import.js
    Expected: npm 包成功导入
    Evidence: .sisyphus/evidence/phase4/task-4-8-npm.txt
  ```

  **Commit**: YES | Message: `feat: implement npm package resolution` | Files: [`crates/wjsm-loader/src/npm.rs`]

### 阶段 5：JIT 编译器和 TypeScript 类型检查

> 目标：实现 JIT 编译器（动态代码即时编译）和完整 TypeScript 类型检查
> 预估：120+ 任务，6-9 个月
> 依赖：阶段 4 完成

- [ ] 5.1 实现 JIT 编译器基础设施

  **What to do**:
  - 设计 JIT 编译器架构（解析 → IR → 机器码）
  - 选择 JIT 代码生成方案（Cranelift/llvm/自研）
  - 实现 JIT 上下文管理
  - 实现代码缓存（热点代码缓存）
  - 集成到运行时

  **Must NOT do**:
  - 不实现复杂优化（内联、逃逸分析）

  **Recommended Agent Profile**:
  - Category: `ultrabrain`
  - Skills: [`coding-guidelines`]

  **Parallelization**: Can Parallel: YES | Wave: 1 | Blocks: [5.2, 5.3] | Blocked By: [阶段4]

  **References**:
  - 理论: JIT 编译原理
  - 实现: Cranelift、V8 TurboFan

  **Acceptance Criteria**:
  - [ ] JIT 可生成机器码
  - [ ] 简单函数可 JIT 编译执行

  **QA Scenarios**:
  ```
  Scenario: JIT 基础
    Tool: Bash
    Steps: cargo nextest run --color=never -p wjsm-backend-jit jit_basic
    Expected: 测试通过
    Evidence: .sisyphus/evidence/phase5/task-5-1-jit-basic.txt
  ```

  **Commit**: YES | Message: `feat(backend-jit): implement JIT compiler infrastructure` | Files: [`crates/wjsm-backend-jit/src/lib.rs`]

- [ ] 5.2 实现 eval() 的 JIT 编译

  **What to do**:
  - 实现 eval() 运行时函数
  - 解析参数字符串为 AST
  - JIT 编译 AST 到机器码
  - 在当前作用域执行 JIT 代码
  - 返回值到 eval 调用点

  **Must NOT do**:
  - 不实现间接 eval（var 不泄漏到全局）

  **Recommended Agent Profile**:
  - Category: `ultrabrain`
  - Skills: [`coding-guidelines`]

  **Parallelization**: Can Parallel: YES | Wave: 2 | Blocks: [5.3] | Blocked By: [5.1]

  **References**:
  - JS 语义: ECMAScript 19.2.1 eval(x)

  **Acceptance Criteria**:
  - [ ] `eval("1 + 1")` 返回 2
  - [ ] `eval("var x = 1")` 在当前作用域创建变量

  **QA Scenarios**:
  ```
  Scenario: eval JIT
    Tool: Bash
    Steps: cargo run -- run fixtures/jit/eval_basic.js
    Expected: eval 执行正确
    Evidence: .sisyphus/evidence/phase5/task-5-2-eval.txt
  ```

  **Commit**: YES | Message: `feat(backend-jit): implement eval() JIT compilation` | Files: [`crates/wjsm-backend-jit/src/eval.rs`]

- [ ] 5.3 实现 new Function() 的 JIT 编译

  **What to do**:
  - 实现 Function 构造函数
  - 解析参数字符串为函数参数和体
  - JIT 编译函数体
  - 创建函数对象（闭包环境为全局）
  - 返回可调用函数

  **Must NOT do**:
  - 不实现所有 Function.prototype 方法

  **Recommended Agent Profile**:
  - Category: `unspecified-high`
  - Skills: [`coding-guidelines`]

  **Parallelization**: Can Parallel: YES | Wave: 2 | Blocks: [5.4] | Blocked By: [5.1]

  **References**:
  - JS 语义: ECMAScript 20.2.1.1 Function(p1, p2, ..., body)

  **Acceptance Criteria**:
  - [ ] `new Function("a", "b", "return a + b")` 工作
  - [ ] 创建的函数可调用

  **QA Scenarios**:
  ```
  Scenario: new Function
    Tool: Bash
    Steps: cargo run -- run fixtures/jit/new_function.js
    Expected: 动态创建函数工作
    Evidence: .sisyphus/evidence/phase5/task-5-3-new-function.txt
  ```

  **Commit**: YES | Message: `feat(backend-jit): implement new Function() JIT` | Files: [`crates/wjsm-backend-jit/src/function_ctor.rs`]

- [ ] 5.4 实现 AOT+JIT 统一运行时对象模型

  **What to do**:
  - 确保 AOT 和 JIT 代码共享相同的对象表示
  - 统一函数调用约定（AOT 可调 JIT，反之亦然）
  - 共享堆和 GC
  - 统一异常处理
  - 性能测试：切换开销 < 10%
  - 编写 AOT/JIT 边界接口文档

  **Must NOT do**:
  - 不实现 AOT 和 JIT 代码内联互调

  **Recommended Agent Profile**:
  - Category: `ultrabrain`
  - Skills: [`coding-guidelines`]

  **Parallelization**: Can Parallel: NO | Wave: 3 | Blocks: [5.5] | Blocked By: [5.2, 5.3]

  **References**:
  - 设计文档: `crates/wjsm-runtime/docs/jit-bridge.md`

  **Acceptance Criteria**:
  - [ ] AOT 代码可调 JIT 函数
  - [ ] JIT 代码可调 AOT 函数
  - [ ] 共享对象修改对双方可见
  - [ ] 设计文档 `crates/wjsm-runtime/docs/jit-bridge.md` 存在

  **QA Scenarios**:
  ```
  Scenario: AOT-JIT 互调
    Tool: Bash
    Steps: cargo run -- run fixtures/jit/aot_jit_interop.js
    Expected: 无缝互操作
    Evidence: .sisyphus/evidence/phase5/task-5-4-aot-jit.txt
  ```

  **Commit**: YES | Message: `feat: unify AOT and JIT runtime object model` | Files: [`crates/wjsm-runtime/src/jit_bridge.rs`, `crates/wjsm-runtime/docs/jit-bridge.md`]

- [ ] 5.5 实现 `check` CLI 子命令

  **What to do**:
  - 在 CLI 添加 `check` 子命令
  - 支持 `wjsm check <file.ts>` 类型检查模式
  - 解析并检查文件但不执行
  - 输出类型错误
  - 返回非零 exit code 当有错误

  **Must NOT do**:
  - 不实现完整类型检查逻辑（调用 tsc crate）

  **Recommended Agent Profile**:
  - Category: `quick`
  - Skills: [`coding-guidelines`]

  **Parallelization**: Can Parallel: YES | Wave: 3 | Blocks: [5.6, 5.7, 5.8] | Blocked By: [1.7]

  **References**:
  - Deno CLI: `deno check` 命令

  **Acceptance Criteria**:
  - [ ] `cargo run -- check file.ts` 可执行
  - [ ] 命令存在并调用类型检查器

  **QA Scenarios**:
  ```
  Scenario: check 命令存在
    Tool: Bash
    Steps: cargo run -- check --help
    Expected: 显示 check 命令帮助
    Evidence: .sisyphus/evidence/phase5/task-5-5-check-help.txt
  ```

  **Commit**: YES | Message: `feat(cli): add check subcommand` | Files: [`crates/wjsm-cli/src/commands/check.rs`]

- [ ] 5.6 实现 TypeScript 解析器增强

  **What to do**:
  - 配置 SWC 保留类型信息（不只是擦除）
  - 构建 TypeScript AST（TAST）
  - 实现类型节点访问者
  - 支持：类型注解、接口、类型别名、泛型

  **Must NOT do**:
  - 不实现类型检查（阶段 5.7）
  - 不实现声明文件（.d.ts）处理（阶段 5.8）

  **Recommended Agent Profile**:
  - Category: `unspecified-high`
  - Skills: [`coding-guidelines`]

  **Parallelization**: Can Parallel: YES | Wave: 4 | Blocks: [5.7] | Blocked By: [5.5]

  **References**:
  - SWC: TypeScript 解析配置

  **Acceptance Criteria**:
  - [ ] TS 类型注解可解析保留
  - [ ] 接口和类型别名可解析

  **QA Scenarios**:
  ```
  Scenario: TS AST
    Tool: Bash
    Steps: cargo nextest run --color=never -p wjsm-parser ts_ast
    Expected: 测试通过
    Evidence: .sisyphus/evidence/phase5/task-5-6-ts-ast.txt
  ```

  **Commit**: YES | Message: `feat(parser): enhance TypeScript AST retention` | Files: [`crates/wjsm-parser/src/ts.rs`]

- [ ] 5.7 实现 TypeScript 类型检查器（基础）

  **What to do**:
  - 实现类型系统核心：类型字面量、基本类型、联合类型
  - 实现类型推断（变量、表达式）
  - 实现类型检查（赋值、函数调用）
  - 支持泛型基础
  - 类型错误报告

  **Must NOT do**:
  - 不实现所有高级类型（条件类型、映射类型）
  - 不实现严格 null 检查（阶段 5.9）

  **Recommended Agent Profile**:
  - Category: `ultrabrain`
  - Skills: [`coding-guidelines`]

  **Parallelization**: Can Parallel: YES | Wave: 5 | Blocks: [5.8, 5.9] | Blocked By: [5.6]

  **References**:
  - 理论: TypeScript 类型系统规范
  - 实现: 参考 tsc 实现

  **Acceptance Criteria**:
  - [ ] `let x: number = 1` 无错误
  - [ ] `let y: string = 1` 报告类型错误

  **QA Scenarios**:
  ```
  Scenario: 类型检查
    Tool: Bash
    Steps: cargo run -- check fixtures/types/basic.ts
    Expected: 类型错误正确报告
    Evidence: .sisyphus/evidence/phase5/task-5-7-type-check.txt
  ```

  **Commit**: YES | Message: `feat: implement TypeScript type checker foundation` | Files: [`crates/wjsm-tsc/src/checker.rs`]

- [ ] 5.8 实现声明文件（.d.ts）处理

  **What to do**:
  - 实现 .d.ts 文件解析
  - 实现声明合并
  - 支持第三方库类型定义
  - 支持 @types 包

  **Must NOT do**:
  - 不实现全局声明自动发现

  **Recommended Agent Profile**:
  - Category: `unspecified-high`
  - Skills: [`coding-guidelines`]

  **Parallelization**: Can Parallel: YES | Wave: 5 | Blocks: [5.9] | Blocked By: [5.7]

  **References**:
  - TypeScript: 声明文件规范

  **Acceptance Criteria**:
  - [ ] `.d.ts` 文件可被加载
  - [ ] 第三方库类型可用

  **QA Scenarios**:
  ```
  Scenario: 声明文件
    Tool: Bash
    Steps: cargo run -- check fixtures/types/with_dts.ts
    Expected: 类型定义正确加载
    Evidence: .sisyphus/evidence/phase5/task-5-8-dts.txt
  ```

  **Commit**: YES | Message: `feat(tsc): implement .d.ts declaration file support` | Files: [`crates/wjsm-tsc/src/declarations.rs`]

- [ ] 5.9 实现 tsconfig.json 支持

  **What to do**:
  - 实现 tsconfig.json 解析
  - 支持：compilerOptions（target、module、strict 等）
  - 支持 include/exclude/files
  - 项目引用（project references）基础
  - 增量编译（incremental）基础

  **Must NOT do**:
  - 不实现复杂构建场景

  **Recommended Agent Profile**:
  - Category: `unspecified-high`
  - Skills: [`coding-guidelines`]

  **Parallelization**: Can Parallel: YES | Wave: 6 | Blocks: [阶段6] | Blocked By: [5.8]

  **References**:
  - TypeScript: tsconfig.json 规范

  **Acceptance Criteria**:
  - [ ] tsconfig.json 可被解析
  - [ ] 配置影响类型检查

  **QA Scenarios**:
  ```
  Scenario: tsconfig
    Tool: Bash
    Steps: cargo run -- check --project fixtures/tsconfig/basic/
    Expected: 配置生效
    Evidence: .sisyphus/evidence/phase5/task-5-9-tsconfig.txt
  ```

  **Commit**: YES | Message: `feat(tsc): implement tsconfig.json support` | Files: [`crates/wjsm-tsc/src/tsconfig.rs`]

### 阶段 6：标准库、工具链和宿主 API

> 目标：实现 Node.js 内置模块、Web Platform API、CLI 工具链、LSP 服务器
> 预估：150+ 任务，6-9 个月
> 依赖：阶段 5 完成

- [ ] 6.1 设计宿主 ABI

  **What to do**:
  - 定义宿主函数调用约定
  - 设计窄而稳定的 host ABI 接口
  - 分类：console、timers、fs、net、http、process 等
  - 参数编码/解码规范
  - 错误映射规范

  **Must NOT do**:
  - 不实现具体宿主函数（阶段 6.2+）

  **Recommended Agent Profile**:
  - Category: `unspecified-high`
  - Skills: [`coding-guidelines`]

  **Parallelization**: Can Parallel: YES | Wave: 1 | Blocks: [6.2, 6.3, 6.4, 6.5, 6.6, 6.7] | Blocked By: [阶段5]

  **References**:
  - 设计: host-abi.md 文档
  - 参考: WASI、Node.js N-API

  **Acceptance Criteria**:
  - [ ] 宿主 ABI 设计文档
  - [ ] 接口定义代码（Rust trait/struct）

  **QA Scenarios**:
  ```
  Scenario: ABI 编译
    Tool: Bash
    Steps: cargo build -p wjsm-host
    Expected: 编译成功
    Evidence: .sisyphus/evidence/phase6/task-6-1-abi.txt
  ```

  **Commit**: YES | Message: `docs: design host ABI specification` | Files: [`crates/wjsm-host/docs/abi.md`, `crates/wjsm-host/src/abi.rs`]

- [ ] 6.2 实现 console 完整 API

  **What to do**:
  - 实现 console.log/info/warn/error
  - 实现 console.table/time/timeEnd
  - 实现格式化输出（%s %d %o 等）
  - 支持多参数
  - 支持对象/数组美化打印

  **Must NOT do**:
  - 不实现 console 的所有方法（阶段 6.8）

  **Recommended Agent Profile**:
  - Category: `unspecified-high`
  - Skills: [`coding-guidelines`]

  **Parallelization**: Can Parallel: YES | Wave: 2 | Blocks: [6.8] | Blocked By: [6.1]

  **References**:
  - 当前: `console_log` 仅支持单参数
  - WhatWG: Console 标准

  **Acceptance Criteria**:
  - [ ] `console.log("a", "b")` 输出多参数
  - [ ] `console.log({a:1})` 美化输出对象

  **QA Scenarios**:
  ```
  Scenario: console 完整
    Tool: Bash
    Steps: cargo run -- run fixtures/api/console_full.js
    Expected: 正确格式化输出
    Evidence: .sisyphus/evidence/phase6/task-6-2-console.txt
  ```

  **Commit**: YES | Message: `feat(api): implement complete console API` | Files: [`crates/wjsm-host/src/console.rs`]

- [ ] 6.3 实现 fs 模块（基础）

  **What to do**:
  - 实现 readFile/readFileSync
  - 实现 writeFile/writeFileSync
  - 实现 exists/access
  - 实现 mkdir/readdir
  - 宿主 ABI 提供文件系统操作

  **Must NOT do**:
  - 不实现流式 API（阶段 6.8）
  - 不实现 watch

  **Recommended Agent Profile**:
  - Category: `unspecified-high`
  - Skills: [`coding-guidelines`]

  **Parallelization**: Can Parallel: YES | Wave: 2 | Blocks: [6.8] | Blocked By: [6.1]

  **References**:
  - Node.js: fs 模块 API

  **Acceptance Criteria**:
  - [ ] `fs.readFileSync("file.txt")` 工作
  - [ ] `fs.writeFileSync("file.txt", "data")` 工作

  **QA Scenarios**:
  ```
  Scenario: fs 基础
    Tool: Bash
    Steps: cargo run -- run fixtures/api/fs_basic.js
    Expected: 文件读写正确
    Evidence: .sisyphus/evidence/phase6/task-6-3-fs.txt
  ```

  **Commit**: YES | Message: `feat(api): implement fs module basics` | Files: [`crates/wjsm-host/src/fs.rs`]

- [ ] 6.4 实现 path 模块

  **What to do**:
  - 实现 path.join/resolve/relative
  - 实现 path.dirname/basename/extname
  - 实现 path.parse/format
  - 实现 path.sep/delimiter
  - 跨平台路径处理（Windows/Unix）

  **Must NOT do**:
  - 无（path 模块可完整实现）

  **Recommended Agent Profile**:
  - Category: `quick`
  - Skills: [`coding-guidelines`]

  **Parallelization**: Can Parallel: YES | Wave: 2 | Blocks: [6.8] | Blocked By: [6.1]

  **References**:
  - Node.js: path 模块 API

  **Acceptance Criteria**:
  - [ ] `path.join("a", "b")` 正确
  - [ ] 跨平台测试通过

  **QA Scenarios**:
  ```
  Scenario: path
    Tool: Bash
    Steps: cargo run -- run fixtures/api/path.js
    Expected: 路径操作正确
    Evidence: .sisyphus/evidence/phase6/task-6-4-path.txt
  ```

  **Commit**: YES | Message: `feat(api): implement path module` | Files: [`crates/wjsm-std/src/path.rs`]

- [ ] 6.5 实现 process 模块（基础）

  **What to do**:
  - 实现 process.argv
  - 实现 process.env
  - 实现 process.exit
  - 实现 process.cwd/chdir
  - 实现 process.version/versions

  **Must NOT do**:
  - 不实现 process 事件（exit/uncaughtException）
  - 不实现 child_process（阶段 6.8）

  **Recommended Agent Profile**:
  - Category: `unspecified-high`
  - Skills: [`coding-guidelines`]

  **Parallelization**: Can Parallel: YES | Wave: 2 | Blocks: [6.8] | Blocked By: [6.1]

  **References**:
  - Node.js: process 模块 API

  **Acceptance Criteria**:
  - [ ] `process.env.PATH` 可用
  - [ ] `process.exit(1)` 工作

  **QA Scenarios**:
  ```
  Scenario: process 基础
    Tool: Bash
    Steps: cargo run -- run fixtures/api/process_basic.js
    Expected: 进程信息正确
    Evidence: .sisyphus/evidence/phase6/task-6-5-process.txt
  ```

  **Commit**: YES | Message: `feat(api): implement process module basics` | Files: [`crates/wjsm-host/src/process.rs`]

- [ ] 6.6 实现 fetch/Web API

  **What to do**:
  - 实现 fetch 函数（使用宿主 HTTP 客户端）
  - 实现 Request/Response/Headers
  - 实现 URL/URLSearchParams
  - 实现 TextEncoder/TextDecoder
  - 实现 WebCrypto 基础（getRandomValues）

  **Must NOT do**:
  - 不实现所有 WebCrypto 算法（阶段 6.8）
  - 不实现 WebSocket（阶段 6.8）

  **Recommended Agent Profile**:
  - Category: `unspecified-high`
  - Skills: [`coding-guidelines`]

  **Parallelization**: Can Parallel: YES | Wave: 2 | Blocks: [6.8] | Blocked By: [6.1]

  **References**:
  - WhatWG: fetch 规范
  - Web Platform APIs

  **Acceptance Criteria**:
  - [ ] `fetch("https://example.com")` 工作
  - [ ] `new URL("http://a/b")` 工作

  **QA Scenarios**:
  ```
  Scenario: fetch
    Tool: Bash
    Steps: cargo run -- run fixtures/api/fetch.js
    Expected: HTTP 请求成功
    Evidence: .sisyphus/evidence/phase6/task-6-6-fetch.txt
  ```

  **Commit**: YES | Message: `feat(api): implement fetch and basic Web APIs` | Files: [`crates/wjsm-host/src/fetch.rs`, `crates/wjsm-std/src/url.rs`]

- [ ] 6.7 实现 CLI 工具链（fmt、lint、test、bench）

  **What to do**:
  - 实现 `wjsm fmt`（格式化，可用外部工具如 dprint）
  - 实现 `wjsm lint`（静态分析，可用外部工具）
  - 实现 `wjsm test`（测试运行器）
  - 实现 `wjsm bench`（基准测试）
  - 统一 CLI 接口设计

  **Must NOT do**:
  - 不实现复杂测试框架（jest 等价物）

  **Recommended Agent Profile**:
  - Category: `unspecified-high`
  - Skills: [`coding-guidelines`]

  **Parallelization**: Can Parallel: YES | Wave: 3 | Blocks: [6.8, 6.9] | Blocked By: [6.2, 6.3, 6.4, 6.5, 6.6]

  **References**:
  - Deno: CLI 设计
  - Node.js: npm test

  **Acceptance Criteria**:
  - [ ] `wjsm fmt file.ts` 工作
  - [ ] `wjsm test` 运行测试

  **QA Scenarios**:
  ```
  Scenario: CLI 工具
    Tool: Bash
    Steps: |
      cargo run -- fmt fixtures/happy/hello.js
      cargo run -- test fixtures/test/
    Expected: 命令成功执行
    Evidence: .sisyphus/evidence/phase6/task-6-7-cli.txt
  ```

  **Commit**: YES | Message: `feat(cli): implement fmt, lint, test, bench commands` | Files: [`crates/wjsm-cli/src/commands/`]

- [ ] 6.8 实现剩余 Node.js 内置模块和 Web API

  **What to do**:
  - 实现 stream 模块
  - 实现 crypto 完整模块（或 WebCrypto 完整）
  - 实现 http/https 服务器
  - 实现 net 模块（socket）
  - 实现 child_process
  - 实现 worker_threads 基础
  - 实现 WebSocket
  - 实现 EventTarget/Event

  **Must NOT do**:
  - 不实现所有 Node.js 模块（优先高频）

  **Recommended Agent Profile**:
  - Category: `unspecified-high`
  - Skills: [`coding-guidelines`]

  **Parallelization**: Can Parallel: YES | Wave: 4 | Blocks: [6.9] | Blocked By: [6.2, 6.3, 6.7]

  **References**:
  - Node.js: 所有核心模块 API

  **Acceptance Criteria**:
  - [ ] http 服务器可启动
  - [ ] stream 可工作
  - [ ] 覆盖 >80% Node.js 常用模块

  **QA Scenarios**:
  ```
  Scenario: http 服务器
    Tool: Bash
    Steps: cargo run -- run fixtures/api/http_server.js &
           sleep 1 && curl http://localhost:3000
    Expected: 服务器响应
    Evidence: .sisyphus/evidence/phase6/task-6-8-http.txt
  ```

  **Commit**: YES | Message: `feat(api): implement remaining Node.js and Web APIs` | Files: [`crates/wjsm-host/src/`, `crates/wjsm-std/src/`]

- [ ] 6.9 实现 LSP 服务器

  **What to do**:
  - 实现 Language Server Protocol 服务器
  - 支持：hover、go-to-definition、completions
  - 支持：diagnostics（类型错误）
  - 支持：document symbols、workspace symbols
  - VS Code 扩展（可选）

  **Must NOT do**:
  - 不实现所有 LSP 功能（优先高频）

  **Recommended Agent Profile**:
  - Category: `ultrabrain`
  - Skills: [`coding-guidelines`]

  **Parallelization**: Can Parallel: YES | Wave: 4 | Blocks: [阶段7] | Blocked By: [6.7]

  **References**:
  - LSP 规范
  - Deno LSP 实现

  **Acceptance Criteria**:
  - [ ] LSP 服务器可启动
  - [ ] VS Code 可连接
  - [ ] hover 显示类型信息

  **QA Scenarios**:
  ```
  Scenario: LSP 服务器可启动
    Tool: Bash
    Steps: |
      # 启动 LSP 服务器并测试基本响应
      timeout 3 cargo run --bin wjsm-lsp -- --version 2>&1 || true
      # 检查帮助信息
      cargo run --bin wjsm-lsp -- --help 2>&1 | grep -i "lsp\|language"
    Expected: |
      - LSP 服务器可执行
      - 显示版本或帮助信息
    Evidence: .sisyphus/evidence/phase6/task-6-9-lsp-version.txt

  Scenario: LSP stdio 模式
    Tool: Bash
    Steps: |
      # 使用简单脚本测试 stdio 模式（发送初始化请求）
      cat <<'EOF' > /tmp/test_lsp.sh
      #!/bin/bash
      # 发送 Content-Length 格式的 LSP 初始化请求
      MSG='{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"processId":1,"rootUri":"file:///tmp","capabilities":{}}}'
      LEN=${#MSG}
      echo -e "Content-Length: $LEN\r\n\r\n$MSG"
      EOF
      chmod +x /tmp/test_lsp.sh
      timeout 5 /tmp/test_lsp.sh | cargo run --bin wjsm-lsp -- stdio 2>/dev/null | head -c 500 | grep -o '"capabilities"' || echo "LSP server started but may not respond (expected for minimal implementation)"
    Expected: |
      - LSP 服务器接受 stdio 输入
      - 可能返回 capabilities（或至少不崩溃）
    Evidence: .sisyphus/evidence/phase6/task-6-9-lsp-stdio.txt
  ```

  **Commit**: YES | Message: `feat: implement LSP server` | Files: [`crates/wjsm-lsp/src/`]

### 阶段 7：生产就绪

> 目标：性能优化、安全沙箱、Node-API 兼容、完整测试覆盖
> 预估：80+ 任务，3-6 个月
> 依赖：阶段 6 完成

- [ ] 7.1 集成 Test262 测试套件

  **What to do**:
  - 下载并配置 Test262
  - 实现 Test262 运行器适配器
  - 运行基础测试套件
  - 实现跳过/标记已知失败机制
  - CI 集成 Test262

  **Must NOT do**:
  - 不要求 100% 通过（阶段 7.4 目标）

  **Recommended Agent Profile**:
  - Category: `unspecified-high`
  - Skills: [`coding-guidelines`]

  **Parallelization**: Can Parallel: YES | Wave: 1 | Blocks: [7.4] | Blocked By: [阶段6]

  **References**:
  - Test262: ECMAScript 官方测试套件
  - Deno Test262 集成

  **Acceptance Criteria**:
  - [ ] Test262 可运行
  - [ ] 基础测试通过率 >50%

  **QA Scenarios**:
  ```
  Scenario: Test262
    Tool: Bash
    Steps: cargo run -- test262 --suite test262/test/
    Expected: 测试套件运行
    Evidence: .sisyphus/evidence/phase7/task-7-1-test262.txt
  ```

  **Commit**: YES | Message: `test: integrate Test262 conformance suite` | Files: [`tests/test262/`]

- [ ] 7.2 实现 npm 包 canary 测试

  **What to do**:
  - 选择 1000+ 常用 npm 包
  - 实现自动化 canary 测试
  - 测试包安装、导入、基本功能
  - 记录通过率
  - CI 集成

  **Must NOT do**:
  - 不要求所有包通过（阶段 7.4 目标）

  **Recommended Agent Profile**:
  - Category: `unspecified-high`
  - Skills: [`coding-guidelines`]

  **Parallelization**: Can Parallel: YES | Wave: 1 | Blocks: [7.4] | Blocked By: [阶段6]

  **References**:
  - npm 包排行榜
  - Deno npm 兼容性测试

  **Acceptance Criteria**:
  - [ ] canary 测试框架工作
  - [ ] 至少 100 个包可测试

  **QA Scenarios**:
  ```
  Scenario: canary
    Tool: Bash
    Steps: cargo run -- canary --packages fixtures/canary/packages.txt
    Expected: 包测试运行
    Evidence: .sisyphus/evidence/phase7/task-7-2-canary.txt
  ```

  **Commit**: YES | Message: `test: implement npm package canary testing` | Files: [`tests/canary/`]

- [ ] 7.3 实现 Node-API 兼容层

  **What to do**:
  - 研究 Node-API（N-API）接口规范
  - 实现 C ABI 兼容层
  - 支持加载原生 addon（.node 文件）
  - 实现 N-API 函数子集（优先高频）
  - 测试原生 addon 加载

  **Must NOT do**:
  - 不实现所有 N-API 函数

  **Recommended Agent Profile**:
  - Category: `ultrabrain`
  - Skills: [`coding-guidelines`]

  **Parallelization**: Can Parallel: YES | Wave: 2 | Blocks: [7.4] | Blocked By: [阶段6]

  **References**:
  - Node.js: Node-API 文档
  - Deno Node-API 实现

  **Acceptance Criteria**:
  - [ ] 简单 .node addon 可加载
  - [ ] addon 函数可调用

  **QA Scenarios**:
  ```
  Scenario: N-API
    Tool: Bash
    Steps: cargo run -- run fixtures/napi/basic.js
    Expected: 原生 addon 工作
    Evidence: .sisyphus/evidence/phase7/task-7-3-napi.txt
  ```

  **Commit**: YES | Message: `feat: implement Node-API compatibility layer` | Files: [`crates/wjsm-napi/src/`]

- [ ] 7.4 实现完整兼容目标

  **What to do**:
  - Test262 通过率 >95%
  - npm canary 通过率 >80%（1000+ 包）
  - Node.js API 覆盖率 >90%
  - 修复剩余兼容性问题
  - 文档化已知不兼容项

  **Must NOT do**:
  - 不追求 100%（文档化例外）

  **Recommended Agent Profile**:
  - Category: `ultrabrain`
  - Skills: [`coding-guidelines`]

  **Parallelization**: Can Parallel: NO | Wave: 3 | Blocks: [7.5, 7.6] | Blocked By: [7.1, 7.2, 7.3]

  **References**:
  - 各测试套件结果

  **Acceptance Criteria**:
  - [ ] Test262 >95%
  - [ ] npm canary >80%

  **QA Scenarios**:
  ```
  Scenario: 兼容性测试
    Tool: Bash
    Steps: |
      cargo run -- test262 --report
      cargo run -- canary --report
    Expected: 达到目标通过率
    Evidence: .sisyphus/evidence/phase7/task-7-4-compatibility.txt
  ```

  **Commit**: YES | Message: `feat: achieve compatibility targets` | Files: [`COMPATIBILITY.md`]

- [ ] 7.5 实现安全沙箱和权限模型

  **What to do**:
  - 实现权限系统（--allow-read、--allow-write、--allow-net 等）
  - 实现权限提示（prompt）
  - 实现权限代理（permission broker）
  - 默认无权限（secure by default）
  - 权限配置（deno.json 等价物）

  **Must NOT do**:
  - 不实现复杂策略引擎

  **Recommended Agent Profile**:
  - Category: `unspecified-high`
  - Skills: [`coding-guidelines`]

  **Parallelization**: Can Parallel: YES | Wave: 4 | Blocks: [7.6] | Blocked By: [7.4]

  **References**:
  - Deno: 权限模型
  - Chrome: Site Isolation

  **Acceptance Criteria**:
  - [ ] 无权限时文件读取被拒绝
  - [ ] --allow-read 后文件读取允许

  **QA Scenarios**:
  ```
  Scenario: 权限
    Tool: Bash
    Steps: |
      cargo run -- run fixtures/security/no_perm.js  # 应失败
      cargo run -- run --allow-read fixtures/security/read_perm.js  # 应成功
    Expected: 权限控制正确
    Evidence: .sisyphus/evidence/phase7/task-7-5-permissions.txt
  ```

  **Commit**: YES | Message: `feat: implement permission model and security sandbox` | Files: [`crates/wjsm-cli/src/permissions.rs`]

- [ ] 7.6 性能优化和调优

  **What to do**:
  - 实现基准测试套件
  - 性能分析和优化（profiling）
  - 优化热点：属性访问、函数调用、GC
  - 实现内联缓存（inline cache）
  - 实现隐藏类（hidden classes/shapes）
  - 达到 Deno 80% 性能目标

  **Must NOT do**:
  - 不实现超标量优化
  - 不实现 SIMD 优化（可选）

  **Recommended Agent Profile**:
  - Category: `ultrabrain`
  - Skills: [`coding-guidelines`]

  **Parallelization**: Can Parallel: YES | Wave: 4 | Blocks: [] | Blocked By: [7.4]

  **References**:
  - V8: 优化编译器
  - Deno: 性能基准

  **Acceptance Criteria**:
  - [ ] 基准测试运行
  - [ ] 达到 Deno 80% 性能

  **QA Scenarios**:
  ```
  Scenario: 性能
    Tool: Bash
    Steps: cargo run -- bench benchmarks/octane/
    Expected: 性能达到目标
    Evidence: .sisyphus/evidence/phase7/task-7-6-performance.txt
  ```

  **Commit**: YES | Message: `perf: optimize runtime performance` | Files: [`crates/wjsm-runtime/src/optimization.rs`]

## Final Verification Wave

> **MANDATORY**: 4 审查代理并行运行，全部通过才能标记完成

- [ ] F1. 计划合规审查 — oracle

  **What to verify**:
  - 所有架构决策符合 Oracle 建议
  - 关键路径依赖正确排序
  - 防护栏（Must NOT Have）被遵守
  - 范围边界清晰

  **Acceptance Criteria**:
  - [ ] Oracle 审查通过

  **QA Scenarios**:
  ```
  Scenario: Oracle 合规审查
    Tool: Bash
    Steps: |
      # 验证架构决策文档存在
      # IR 设计文档
      ls -la crates/wjsm-ir/docs/ir-design.md 2>/dev/null && echo "✓ IR design doc exists"
      # Host ABI 文档
      ls -la crates/wjsm-host/docs/abi.md 2>/dev/null && echo "✓ Host ABI doc exists"
      # AOT/JIT 边界文档
      ls -la crates/wjsm-runtime/docs/jit-bridge.md 2>/dev/null && echo "✓ AOT/JIT bridge doc exists"
      # 验证关键路径依赖
      grep -A 20 "## Execution Strategy" .sisyphus/plans/wjsm-production-ready.md | head -25
    Expected: |
      - 架构文档存在
      - 关键路径清晰定义
    Evidence: .sisyphus/evidence/final/f1-oracle-review.txt
  ```

- [ ] F2. 代码质量审查 — unspecified-high

  **What to verify**:
  - 代码遵循 Rust 最佳实践
  - 无代码异味（code smells）
  - 测试覆盖率达标
  - 文档完整

  **Acceptance Criteria**:
  - [ ] clippy 无警告
  - [ ] 测试覆盖率 >80%

  **QA Scenarios**:
  ```
  Scenario: Clippy 检查
    Tool: Bash
    Steps: cargo clippy --workspace -- -D warnings
    Expected: 无警告，exit code 0
    Evidence: .sisyphus/evidence/final/f2-clippy.txt

  Scenario: 测试覆盖率
    Tool: Bash
    Steps: |
      cargo tarpaulin --workspace --out Html --output-dir .sisyphus/evidence/final/
      cat .sisyphus/evidence/final/tarpaulin-report.html | grep -o '[0-9]\+%' | head -1
    Expected: 覆盖率 >80%
    Evidence: .sisyphus/evidence/final/f2-coverage.html

  Scenario: 代码质量检查
    Tool: Bash
    Steps: |
      # 检查代码质量：函数长度、圈复杂度
      # 使用 rustfmt 检查格式
      cargo fmt -- --check
      # 检查 crate 结构
      find crates -name 'Cargo.toml' -exec dirname {} \; | sort
    Expected: |
      - 代码格式正确
      - 所有 crate 结构完整
    Evidence: .sisyphus/evidence/final/f2-quality-check.txt
  ```

- [ ] F3. 实际 QA 执行 — unspecified-high

  **What to verify**:
  - 运行完整测试套件
  - Test262 通过率检查
  - npm canary 测试
  - 端到端场景测试

  **Acceptance Criteria**:
  - [ ] `cargo nextest run --color=never` 通过
  - [ ] Test262 >95%
  - [ ] npm canary >80%

  **QA Scenarios**:
  ```
  Scenario: 完整测试套件
    Tool: Bash
    Steps: cargo nextest run --color=never --workspace
    Expected: 所有测试通过
    Evidence: .sisyphus/evidence/final/f3-unit-tests.txt

  Scenario: Test262 测试
    Tool: Bash
    Steps: cargo run --bin wjsm -- test262 --suite test262/test/ --report > .sisyphus/evidence/final/test262-report.txt
    Expected: 通过率 >95%
    Evidence: .sisyphus/evidence/final/f3-test262-report.txt

  Scenario: npm canary 测试
    Tool: Bash
    Steps: cargo run --bin wjsm -- canary --packages tests/canary/top-1000.txt --report > .sisyphus/evidence/final/canary-report.txt
    Expected: 通过率 >80%
    Evidence: .sisyphus/evidence/final/f3-canary-report.txt

  Scenario: 端到端场景测试
    Tool: Bash
    Steps: |
      # 使用 fixtures/api 中的 HTTP 服务器测试（6.8 任务产出）
      cargo run --bin wjsm -- run fixtures/api/http_server.js &
      sleep 2
      curl -s http://localhost:3000/ > /tmp/e2e_response.txt
      kill %1 2>/dev/null || true
      cat /tmp/e2e_response.txt
    Expected: |
      - HTTP 服务器响应正常
      - 响应包含预期内容
    Evidence: .sisyphus/evidence/final/f3-e2e.txt
  ```

- [ ] F4. 范围保真度检查 — deep

  **What to verify**:
  - 交付物与计划一致
  - 无范围蔓延
  - 所有 Must Have 完成
  - Must NOT Have 未被违反

  **Acceptance Criteria**:
  - [ ] 范围检查通过

  **QA Scenarios**:
  ```
  Scenario: Must Have 检查清单
    Tool: Bash
    Steps: |
      # 验证所有 Must Have 交付物存在
      echo "Checking Must Have deliverables..."
      ls target/release/wjsm 2>/dev/null && echo "✓ CLI binary exists" || (cargo build --release --bin wjsm && ls target/release/wjsm)
      ls crates/wjsm-lsp/src/ 2>/dev/null && echo "✓ LSP crate exists"
      ls crates/wjsm-runtime/src/gc.rs 2>/dev/null && echo "✓ GC implemented"
      ls crates/wjsm-backend-jit/src/lib.rs 2>/dev/null && echo "✓ JIT implemented"
      ls crates/wjsm-tsc/src/checker.rs 2>/dev/null && echo "✓ TypeScript checker exists"
      # 检查 Node API 覆盖（检查目录非空）
      ls crates/wjsm-host/src/*.rs 2>/dev/null | wc -l | xargs -I {} echo "✓ {} host API files"
      ls crates/wjsm-std/src/*.rs 2>/dev/null | wc -l | xargs -I {} echo "✓ {} std files"
    Expected: |
      - 所有 Must Have 交付物存在
      - 无 Must NOT Have 违反
    Evidence: .sisyphus/evidence/final/f4-scope-check.txt

  Scenario: 交付物验证
    Tool: Bash
    Steps: |
      # 构建 release 二进制（确保存在）
      cargo build --release --bin wjsm 2>&1 | tail -5
      # 检查主要交付物
      echo "=== Binary ===" 
      ls -lh target/release/wjsm 2>&1
      echo "=== Core Crates ==="
      ls -la crates/wjsm-cli/ crates/wjsm-parser/ crates/wjsm-semantic/ crates/wjsm-ir/ crates/wjsm-runtime/ 2>&1 | head -20
      echo "=== Backend Crates ==="
      ls -la crates/wjsm-backend-wasm/ crates/wjsm-backend-jit/ 2>&1 | head -10
      echo "=== Module System ==="
      ls -la crates/wjsm-loader/ 2>&1 | head -10
      echo "Check npm support:"
      find crates/wjsm-loader/src -name "*.rs" -exec grep -l "npm" {} \; 2>/dev/null | head -3
      echo "=== TypeScript Support ==="
      ls -la crates/wjsm-tsc/ 2>&1
      echo "=== Host APIs ==="
      ls -la crates/wjsm-host/ crates/wjsm-std/ 2>&1 | head -10
      echo "=== LSP ==="
      ls -la crates/wjsm-lsp/ 2>&1
      echo "=== Documentation ==="
      ls -la README.md 2>&1 || echo "Note: README not found"
      ls -la crates/*/docs/ 2>&1 | head -20
    Expected: |
      - 所有核心 crate 存在
      - 主要文档存在（README）
    Evidence: .sisyphus/evidence/final/f4-deliverables.txt
  ```

## Commit Strategy

- 每个任务独立提交
- 提交信息格式：`type(scope): description`
- type: feat, fix, refactor, test, docs, perf, chore
- scope: 主要 crate 或模块名
- 保持提交小且原子化
- 每个提交必须通过 CI

## Success Criteria

项目成功当且仅当：

1. **功能完整**：
   - [ ] 完整 JavaScript 语义支持（ES2024+）
   - [ ] 完整 TypeScript 支持（解析 + 类型检查）
   - [ ] AOT+JIT 混合编译工作
   - [ ] 完整 npm 生态兼容（含 Node-API）
   - [ ] Node.js 内置模块覆盖率 >90%
   - [ ] Web Platform API 完整
   - [ ] CLI 工具链完整（fmt/lint/test/bench）
   - [ ] LSP 服务器工作

2. **质量标准**：
   - [ ] Test262 通过率 >95%
   - [ ] npm canary 测试通过率 >80%（1000+ 包）
   - [ ] 单元测试覆盖率 >80%
   - [ ] 性能达到 Deno 80%

3. **工程成熟度**：
   - [ ] CI/CD 完整
   - [ ] 文档完整（API、架构、贡献指南）
   - [ ] 安全沙箱和权限模型
   - [ ] 可维护的架构（分层编译器、统一 IR）

4. **生产就绪**：
   - [ ] 稳定发布流程
   - [ ] 向后兼容承诺
   - [ ] 社区贡献指南
   - [ ] 问题追踪和响应流程
