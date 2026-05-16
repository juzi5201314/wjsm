# wjsm 特性优先级

基于 test262 eval 专项验证（2026-05-16）分析得出的特性实现顺序。

## 基础

- [x] NaN-boxed value encoding
- [x] Mark-sweep GC
- [x] IR — 基于 SSA 的中间表示
- [x] WASM 后端 — 基础指令生成
- [x] 运行时 — wasmtime 执行 + host 函数链接
- [x] CLI — `build`/`run` 子命令
- [x] 模块系统 — ESM/CJS 打包
- [x] test262 运行器 — 特性过滤 + 批量测试

## 测试工具

- [x] Fixture runner — E2E + 语义快照
- [x] test262 filt runner — 基于 `SUPPORTED_FEATURES` 过滤

## 已实现语言特性

- 函数（声明/表达式/箭头/参数默认值/rest/spread）
- 闭包（词法捕获、箭头 this）
- async/await、async generator
- Promise（all/race/allSettled/any + microtask）
- 类（ctor、方法、getter/setter、static、private、static block、super）
- 对象（属性描述符、原型链、计算属性、展开）
- 数组（字面量 + Array.prototype 全部方法）
- 模板字面量（标签化）
- 解构（数组、对象、参数、默认值）
- 控制流（if/else、switch、循环、break/continue/labeled、
  try/catch/finally/throw）
- 模块（ES import/export、CommonJS require/exports、dynamic import）
- eval（直接/间接、严格模式、作用域桥接、变量写入、编译缓存）
- BigInt、Symbol、RegExp、Proxy（基础）
- JSX、TypeScript 注解
- `using` 声明（显式资源管理）
- JSON、Object/Array/String 内置方法
- console、timer API、fetch (data: URL)

## 基于 test262 分析的特性优先级

### Tier 1 — 核心 eval 规范补全（高收益）

完成 `eval` 层的语义 gap，使 test262 eval 通过率从 11% 提升至 >80%。

1. **[HIGH] `arguments` 对象在 eval 中的绑定规则**
   - 测试数量：~144 个（占直接 eval 测试 50%）
   - 覆盖：`func-decl-*` `func-expr-*` `meth-*` `gen-*` `async-*` `arrow-fn-*`
   - 场景：当 `eval()` 调用发生在函数体内，eval 代码引用/声明 `arguments`
   - 规格：[PerformEval](https://tc39.es/ecma262/#sec-performeval) 第 15-19 步的 var/lex 声明处理
   - 优先级理由：解决一半的 eval 失败

2. **[HIGH] 块级声明在 eval 中的暂存死区 (TDZ)**
   - 测试数量：7
   - 测试：`lex-env-no-init-{let,const,cls}` `lex-env-distinct-{let,const,cls}`
   - 场景：`eval('typeof C; class C {}')` 应抛出 ReferenceError（C 在 class 声明前处于 TDZ）
   - 规格：[BlockDeclarationInstantiation](https://tc39.es/ecma262/#sec-blockdeclarationinstantiation) — 初始化前 `[[Initialized]]` 为 false

3. **[HIGH] `super` 关键字在 eval 中的传递**
   - 测试数量：10
   - 测试：`super-{prop,call,prop-arrow,call-arrow,call-fn,call-method,prop-dot-no-home,...}`
   - 场景：在方法内调用 `eval('super.prop')` 或 `eval('super()')`
   - 规格：[PerformEval](https://tc39.es/ecma262/#sec-performeval) 中 `env` 的 `[[HomeObject]]` 传递
   - 注意：需要方法上下文的 `[[HomeObject]]` 正确传递到 eval 词法环境

### Tier 2 — 重要 eval 语义

4. **[MEDIUM] `new.target` 在 eval 中的传递**
   - 测试数量：3
   - 测试：`new.target.js` `new.target-fn.js` `new.target-arrow.js`
   - 场景：构造函数内 `eval('new.target')` 应返回构造函数
   - 实现：`new.target` 元属性需要在 eval 编译链中传递

5. **[MEDIUM] eval parse-failure 处理**
   - 测试数量：6
   - 测试：`parse-failure-{1..6}.js`
   - 场景：某些 eval 字符串应产生 SyntaxError
   - 排查：确认哪些语法错误被正确检测，哪些遗漏了

6. **[MEDIUM] 非严格模式 eval 中 var/function 提升特殊规则**
   - 测试数量：~6
   - 测试：`var-env-{func,var}-{non-strict,...}` 中的失败项
   - 场景：非严格模式下 eval 中的同名 `var` 和 `function` 声明提升优先级
   - 规格：[EvalDeclarationInstantiation](https://tc39.es/ecma262/#sec-evaldeclarationinstantiation) var 与 function 的冲突处理

7. **[MEDIUM] `block`/`switch`/`switch-dflt` 声明在 eval 中的行为**
   - 测试数量：9
   - 测试：`block-decl-*` `switch-case-decl-*` `switch-dflt-decl-*`
   - 场景：eval 中块级声明与函数声明的交互
   - 规格：[BlockDeclarationInstantiation](https://tc39.es/ecma262/#sec-blockdeclarationinstantiation) 中的 web-compat 规则

### Tier 3 — 边界情况

8. **[LOW] `with` 语句与 eval 交互**
   - 测试数量：1
   - 测试：`global-env-rec-with.js`
   - 场景：`with` 对象中的变量对 eval 的影响

9. **[LOW] `strictness-override`**
   - 测试数量：1
   - 测试：`strictness-override.js`

10. **[LOW] `cptn-thrw-prim` (throw 的完成值)**
    - 测试数量：1
    - 测试：`cptn-thrw-prim.js`

11. **[LOW] 非可定义变量错误**
    - 测试数量：2
    - 测试：`non-definable-function-with-function.js` `non-definable-function-with-variable.js`

12. **[LOW] Annex B eval 扩展**
    - 测试数量：~469
    - 测试：`test/annexB/language/eval-code/{direct,indirect}/`
    - 场景：Web 兼容性扩展（非严格模式块级函数提升等）

13. **[LOW] eval 中模块导入/导出**
    - 测试数量：2
    - 测试：`export.js` `import.js`
    - 注意：在 `--script` 模式下不适用

## 已知问题

- 95 个 semantic snapshot 测试失败（预先存在，需更新 `.ir` 文件）
- 4 个 wjsm-runtime 编译警告（预先存在）

## 运行 test262 eval 测试

```bash
# 直接 eval 测试
cargo run -p wjsm-test262 -- run --suite test/language/eval-code/direct --all --plain

# 间接 eval 测试
cargo run -p wjsm-test262 -- run --suite test/language/eval-code/indirect --all --plain

# 内置 eval 测试
cargo run -p wjsm-test262 -- run --suite test/built-ins/eval --all --plain

# 单个测试
cargo run -p wjsm-test262 -- run --suite test/language/eval-code/direct/cptn-nrml-empty-if.js --all

# 更新 eval 特性后，将 "eval" 从 config.rs 移除并重新加入以清空缓存
```
