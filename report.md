# feat/arguments Code Review Report

结论：**Request changes**。

本报告基于 `master...feat/arguments` PR-style diff。已重新派发 16 个 reviewer subagent，并对高风险发现做了人工复核与定向运行验证。以下只保留已确认属实、应阻塞合并的问题。

## 阻塞问题

### 1. `switch` 嵌套在分支体内会生成无效 WASM

- 严重级别：major
- 文件：`crates/wjsm-backend-wasm/src/compiler_control.rs`
- 位置：约 `515-540`, `1069-1145`

问题：

- `compile_branch_body_with_context` 新增的 `Terminator::Switch` 分支只发射 nested switch scaffold，然后 `Ok(false)` 返回。
- 没有继续编译 nested `exit_block` / 后续 block。
- 顶层 switch 还新增了对已编译 `exit_idx` 的二次 inline emit，容易重复 exit block 或提前终止结构化编译。

复核证据：

```js
if (true) {
  switch (1) { case 1: console.log(1); break; }
  console.log(2);
}
console.log(3);
```

运行 `cargo run --quiet -- run /tmp/wjsm-review-switch.js` 触发 WASM `unreachable` trap。

建议修复：

- nested switch 逻辑复用/抽出顶层 switch 后续编译路径。
- 不要在 `compiled_blocks` 已包含 `exit_idx` 时再盲目发射 exit instructions。
- 添加 fixture：`if { switch (...) ... }`、switch no-default、switch 后续语句。

### 2. 非严格普通函数的 mapped `arguments.callee` 永远缺失

- 严重级别：major
- 文件：
  - `crates/wjsm-semantic/src/lowerer_declarations.rs`
  - `crates/wjsm-runtime/src/runtime_arguments.rs`
- 位置：约 `lowerer_declarations.rs:435-453`, `runtime_arguments.rs:192-194`

问题：

- lowering 调用 `CreateMappedArgumentsObject` 时第三个参数 `func_ref` 总是 `undefined`。
- runtime 只有 `func_ref != undefined` 才定义 `callee`。
- 结果：sloppy ordinary function 的 `arguments.callee` 是 `undefined`，不符合 ECMAScript 行为。

复核证据：

```js
function f() {
  console.log(typeof arguments.callee);
  console.log(arguments.callee === f);
}
f();
```

当前输出：

```text
undefined
false
```

期望输出：

```text
function
true
```

建议修复：

- 在函数 lowering 中把当前函数引用/闭包引用传入 `CreateMappedArgumentsObject`。
- 添加 fixture：sloppy function `arguments.callee === f`。

### 3. `ArrayPushSpread` / `IteratorFrom` 只调用 native iterator，用户自定义 iterable 被误判不可迭代

- 严重级别：major
- 文件：
  - `crates/wjsm-runtime/src/host_imports/array_object.rs`
  - `crates/wjsm-runtime/src/host_imports/core.rs`
- 位置：约 `array_object.rs:20-26`, `75-84`, `core.rs:303-314`

问题：

- `@@iterator` method 和 iterator `next` 只在 `value::is_native_callable` 时调用。
- WASM/user-defined function callable 直接变成 `undefined`。
- `[...obj]`、`for-of` 的普通自定义 iterable 会失败。

复核证据：

```js
const obj = {
  [Symbol.iterator]() {
    let i = 0;
    return {
      next() {
        i++;
        return { value: i, done: i > 2 };
      }
    };
  }
};
const arr = [...obj];
console.log(arr.length);
console.log(arr[0]);
```

当前运行结果：

```text
0
undefined
Runtime error: TypeError: value is not iterable
```

建议修复：

- 走统一 callable 调用路径，同时支持 native callable 与 WASM callback。
- 正确传播 iterator method / next 的异常。
- 添加 fixture：对象自定义 `[Symbol.iterator]` + user function `next`。

### 4. 动态取得的 Number primitive method 忽略 digits/precision 参数

- 严重级别：major
- 文件：`crates/wjsm-runtime/src/runtime_builtins.rs`
- 位置：约 `1379-1391`

问题：

- `NativeCallable::NumberPrimitiveMethod` 的 `toFixed` / `toExponential` / `toPrecision` 只返回 `format_number_js(x)`。
- 忽略 `args[0]`。
- 静态 fast path `(42).toFixed(2)` 可能走别的 builtin 正确；动态 property path 错。

复核证据：

```js
let n = 42;
let m = n.toFixed;
console.log(m.call(n, 2));
console.log(n["toFixed"](2));
```

当前输出：

```text
42
42
```

期望输出：

```text
42.00
42.00
```

建议修复：

- 让 `NumberPrimitiveMethod` 复用已有 `number_proto_to_fixed` / `to_exponential` / `to_precision` 逻辑，或抽出共享 helper。
- 添加动态 property/call fixture。

### 5. `object_methods_proxy.expected` 把错误行为 bless 成通过

- 严重级别：major
- 文件：
  - `fixtures/happy/object_methods_proxy.js`
  - `fixtures/happy/object_methods_proxy.expected`
- 位置：约 `object_methods_proxy.js:5-14`, `object_methods_proxy.expected:3-10`

问题：

fixture source 的 proxy `ownKeys` trap 应输出 `ownKeys trap`，并返回 target keys `['a','b','c']`；expected 却记录：

```text
keys: ["Symbol.asyncIterator"]
entries: [["Symbol.toStringTag","AsyncGenerator"]]
own names: ["length","name"]
```

这不是 proxy fixture 的语义，像是被其它 prototype/host-object key 污染了。

建议修复：

- 修实现，不要改 snapshot 接受错误输出。
- expected 应断言 trap fired + target-derived keys/entries/names。

### 6. host async objects 同时定义字符串 `"Symbol.asyncIterator"` 和真实 Symbol key

- 严重级别：major
- 文件：
  - `crates/wjsm-runtime/src/host_imports/async_generator.rs`
  - `crates/wjsm-runtime/src/host_imports/streams_readable.rs`
  - `crates/wjsm-runtime/src/host_imports/streams_transform.rs`
- 位置：约 `async_generator.rs:51-63`, `streams_readable.rs:214-222`, `streams_transform.rs:127-135`

问题：

- 新增真实 `encode_symbol_name_id(3)` 后，旧的字符串属性仍保留。
- 结果对象同时有 string-key `"Symbol.asyncIterator"` 和 symbol-key `[Symbol.asyncIterator]`。
- 会污染 `Object.keys` / `Object.getOwnPropertyNames` / `Reflect.ownKeys` / proxy trap key 类型。

建议修复：

- 删除字符串 key 版本，只保留 symbol key。
- 添加断言：`Object.getOwnPropertyNames(obj)` 不包含 `"Symbol.asyncIterator"`，`Object.getOwnPropertySymbols(obj)` 包含 `Symbol.asyncIterator`。

### 7. async compiled eval 没同步 `new.target`

- 严重级别：major
- 文件：`crates/wjsm-runtime/src/runtime_eval.rs`
- 位置：sync path 约 `66-85`；async path 约 `108-163`

问题：

- `try_compiled_eval_from_caller` 会从 scope record 同步 `new_target` 到 `RuntimeState`。
- `try_compiled_eval_from_caller_async` 缺少同一段逻辑。
- async eval path 对 `eval('new.target')` 会和 sync compiled path 不一致。

建议修复：

- 抽出 `sync_eval_new_target_from_scope_record(caller, scope_env)`，sync/async 共同调用。
- 添加 async compiled eval new.target fixture。

## 文档/测试问题

### 8. Aegis work logs 已与实现相矛盾

- 严重级别：minor
- 文件：
  - `docs/aegis/work/2026-06-07-eval-arguments-tdz-super-gaps/10-intent.md`
  - `docs/aegis/work/2026-06-07-eval-arguments-tdz-super-gaps/20-checkpoint.md`
  - `docs/aegis/work/2026-06-07-eval-arguments-tdz-super-gaps/90-evidence.md`
- 位置：约 `10-intent.md:5`, `20-checkpoint.md:6,24`, `90-evidence.md:18,60`

问题：

- 文档仍说 `Symbol.iterator` deferred / host helper only string keys。
- 同时分支已引入 `property_key.rs`、`encode_symbol_name_id`、arguments `@@iterator` fixtures。
- checkpoint 还说 `arguments-callee-strict` 仍 KNOWN-BROKEN；实际已有 happy fixture。

建议修复：

- 更新 work log：symbol-key property support 已落地，但仍有 duplicate string-key bugs。
- 删除 `arguments-callee-strict` KNOWN-BROKEN 表述。

## 未采纳的 subagent 发现

以下发现经复核后暂不作为阻塞项：

- “恢复 eval `var arguments` redeclaration guard”：不采纳。Node 复核显示 sloppy direct eval 中 `var arguments = ...` 是允许的，包括参数名为 `arguments` 的 sloppy function。需要补的是 strict direct eval 覆盖，不是恢复旧的 blanket error。
- “object literal methods 必须设置 `is_method = true`”：不采纳。sloppy object literal method 在 Node 中仍有 mapped arguments；盲目套 class-method unmapped 规则会错。
- “`json_parse_to_string_async` line 709 必须递归 async”：未确认。该分支只在 result 已经是 primitive 时进入，同步转换没有再次调用 JS callback。
- “`builtin_from_number_proto_method` 被放进 promise mapper”：当前文件中没有这个问题。
- “`compile_number_proto_wrappers` 顺序必错”：当前没有找到用户函数编译期依赖这些 host import table reverse mapping 的直接证据，暂不作为阻塞项。

## 复核命令

定向运行过以下命令；这不是全量 gate。

```bash
cargo run --quiet -- run /tmp/wjsm-review-switch.js
cargo run --quiet -- run /tmp/wjsm-review-callee.js
cargo run --quiet -- run /tmp/wjsm-review-iterator.js
cargo run --quiet -- run /tmp/wjsm-review-number-dyn.js
```

另外用 Node spot-check 了 eval `arguments` 语义：sloppy direct eval `var arguments` 允许；strict direct eval `var arguments` 是 SyntaxError。
