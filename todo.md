# wjsm 待实现任务

> ✅ 已完成 | ❌ 未开始

---

## 已完成功能速览

| 领域 | 内容 |
|------|------|
| 函数与类 | `function` 声明/表达式、箭头函数、`class` 声明/表达式、`new`、`this`、一般调用 |
| 运算符 | 算术、比较（`==`/`!=` 跨类型 AbstractEq、`===`/`!==` 严格相等、`<`/`>`/`<=`/`>=` AbstractCompare）、逻辑（`&&`/`||`/`??`）、位运算、一元、三元、逗号、复合赋值、`typeof`/`in`/`instanceof`/`delete`、自增自减 |
| 字面量 | 数字、字符串、`true`/`false`、`null`、`undefined`、对象字面量 `{a:1}`（含 shorthand） |
| 控制流 | `if`/`else`、`switch`（含 fallthrough + 嵌套）、`while`/`do..while`/`for`/`for..in`/`for..of`、`break`/`continue`（含 label）、`return`、`labeled`、`try`/`catch`/`finally`/`throw`、`debugger` |
| 对象系统 | 属性读写 `obj.prop`、属性描述符 flags（configurable/writable/enumerable）、`$obj_set` 扩容、`$obj_delete` configurable 检查、`Object.defineProperty`/`getOwnPropertyDescriptor`、`SetProto` 有效性验证、`func_props` 按需分配、**标记-清除 GC（堆上限 + 定期触发）** |
| 运行时 | NaN-boxed 值编码（f64/string/bool/null/undefined/object/fn/exception/iterator/enumerator）、`$obj_new` 堆分配（**GC 集成**）、原型链遍历 `$obj_get`、`string_concat`、`abstract_eq`（跨类型相等）、`abstract_compare`（关系比较）、`to_number`/`to_primitive` 辅助函数、**影子栈函数调用约定（无限参数）** |
| 宿主 API | `console.log`/`error`/`warn`/`info`/`debug`/`trace`、`setTimeout`/`clearTimeout`/`setInterval`/`clearInterval`**（含事件循环）**、`fetch`**（data: URL）**、`JSON.stringify`/`JSON.parse` |

> `with` 语句明确不支持（已废弃特性）。以下限制已在语义层显式报错：`obj.x++` 成员表达式自增自减、`obj.x += 1` 复合赋值到成员表达式。

---

## 任务列表（按依赖顺序）

### 块 A：运行时基础算子

无外部依赖，可并行执行。

- [x] **完善 `+` 运算符字符串拼接** — 验证 `string_concat` 两阶段逻辑（字符串+字符串、字符串+数字、数字+数字的自动回退），清理 `end_try` 死代码。影响层：运行时
- [x] **`==` 跨类型隐式转换** — 新增 `AbstractEq` 宿主函数，实现 `ToPrimitive` / `ToNumber` 跨类型转换，补齐 `null == undefined` 已实现之外的全部 `==` 语义。影响层：IR（新增 Builtin）+ 运行时
- [x] **`<` `>` `<=` `>=` 扩展比较** — 新增 `AbstractCompare` 宿主函数，补齐字符串字典序比较、`null`/`undefined` 数值转换、对象 `ToPrimitive`。影响层：IR + WASM 后端 + 运行时
- [x] **`for...in` 枚举器支持非字符串值** — 修复 `enumerator_from` 对 `bool` 等非字符串值的问题，布尔值返回空枚举。影响层：运行时

### 块 B：函数调用约定与宿主 API

无外部依赖，可并行执行。

- [x] **函数调用约定扩展（支持 >7 个参数）** — 移除 `args.iter().take(7)` 限制，扩展 WASM 函数类型为多参数或数组传参。影响层：IR + WASM 后端
- [x] **堆分配器改进（bump → 可回收）** — 当前仅简单 bump allocator（`$obj_new`），无法回收；实现基础 GC（标记-清除或引用计数）。注意：闭包 PoC 可用现有 bump allocator 先行实现（只分配不释放）。影响层：运行时
- [x] **宿主 API（console.error / setTimeout / fetch 等）** — 除 `console.log` 外全部缺失。影响层：运行时

### 块 C：闭包与词法作用域 ✅

依赖：块 B 堆分配器（PoC 阶段可用现有 bump allocator 先行）。内部两个任务强耦合，不可拆分。

- [x] **闭包 — 词法变量捕获** — `CreateClosure` IR 指令 + 语义层逃逸分析 + env 对象传递 + WASM 后端闭包调用链路 + 运行时 `closure_create/get_func/get_env`。实现方案：捕获变量通过 env 对象（`NewObject` + `SetProp`/`GetProp`）传递，闭包通过 `TAG_CLOSURE` 标记区分普通函数引用，调用时运行时解析闭包获取 func_idx + env_obj。
- [x] **箭头函数捕获词法 `this`** — 箭头函数内 `this` 通过 env 对象词法捕获，`lower_this` 检测箭头函数上下文后走 `GetProp(env, "$this")` 路径。影响层：语义

### 块 D：数组

依赖：块 B 堆分配器（数组方法 `push` 需动态扩容；字面量可用现有 `$obj_new`）。

- [x] **数组字面量 `[1, 2, 3]`** — 语义层识别 `ArrayLit` AST 节点，生成 `NewArray` + `ArrayPush` builtin；含稀疏数组（`[1, , 3]`）。影响层：语义 + WASM 后端
- [x] **数组方法（push/pop/includes/indexOf/join/fill/reverse/flat/concat/slice）** — 运行时宿主函数，操作堆上数组对象（新内存布局 offset +4）。影响层：运行时
- [x] **数组方法（map/filter/reduce/reduceRight/find/findIndex/some/every/forEach/flatMap/at/copyWithin/sort/splice/shift/unshift/Array.isArray）** — 全部 Array.prototype 方法和 Array.isArray 的运行时宿主函数实现。影响层：IR + WASM 后端 + 运行时
- [x] **数组方法调用优化（语义层拦截）** — 在 `lower_call_expr` 中识别 `a.filter(callback)` 模式，发出 `CallBuiltin` 代替 `Call`，跳过运行时属性解析，直接跳转宿主函数。影响层：语义

### 块 E：模板字符串 ✅

依赖：块 A 字符串拼接（`string_concat` 完善后）。

- [x] **模板字符串 `` `hello ${world}` ``** — 语义层通过 `StringConcatVa` 新 IR 指令实现，一次宿主调用完成全部拼接（非多次 `string_concat`）。含 Tagged Template（专用 lowering，构建 cooked+raw quasi 数组，`Object.defineProperty` 设 raw 属性）。影响层：IR + 语义 + WASM 后端 + 运行时

### 块 F：语法糖与对象增强

无互依赖，可并行执行。

- [x] **可选链 `a?.b`** — 语法糖 lowering，生成 OptionalGetProp/OptionalGetElem/OptionalCall IR 指令，WASM 后端内联 null/undefined 短路检查。影响层：IR + 语义 + WASM 后端
- [x] **`super` 关键字** — 支持 `super.method()` 和 `super.prop`（含计算属性），语义层生成 GetSuperBase + GetProp/GetElem，通过 Function.home_object 传递基类引用。影响层：IR + 语义 + WASM 后端
- [x] **解构（声明 + 函数参数 + 默认值）** — `let {a, b} = obj`、`let [x, y] = arr`、函数参数解构 `function f({a, b})`、参数默认值 `function f(x = 1)`。语义层将解构模式 lowering 为一系列属性访问 + 变量赋值。影响层：语义
- [x] **对象字面量计算属性名 + spread** — `{ [expr]: val }` 通过 lower_prop_name 处理 Computed 键名；`{ ...obj }` 通过 ObjectSpread IR 指令 + 运行时 obj_spread 实现。影响层：IR + 语义 + WASM 后端 + 运行时
- [x] **类 getter/setter 方法** — lower_class_decl/expr 处理 MethodKind::Getter/Setter，构建属性描述符 {get/set, enumerable: false, configurable: true}，通过 DefineProperty 挂载到 prototype 或构造函数。对象字面量 getter/setter 同理（enumerable/configurable 默认 true）。影响层：语义
- [x] **类静态方法 / 静态块** — 静态方法通过 method.is_static 判断，SetProp 到 ctor_dest 而非 proto_dest；`static {}` 静态初始化块创建独立函数并立即 Call(this=ctor)。影响层：语义
- [ ] **`new` 表达式 prototype 查找修复** — 当前仅在函数自身属性对象上搜索 `prototype`，不走完整 `[[Get]]`（不遍历函数原型链）。需 GetPrototypeFromConstructor Builtin（已注册，运行时 stub）。影响层：语义 + 运行时
- [x] **`this` 绑定：call / apply / bind** — 语义层拦截 func.call/apply/bind 转为 CallBuiltin，运行时实现 func_call/func_apply/func_bind + BoundRecord 表 + TAG_BOUND 递归解包。影响层：IR + 语义 + WASM 后端 + 运行时

### 块 G：Object 标准方法

无外部依赖（10.3 已全部完成）。可并行执行。

- [ ] `Object.prototype.hasOwnProperty()` — Builtin 指令或运行时宿主函数
- [ ] `Object.keys()` / `Object.values()` / `Object.entries()` — 遍历自身可枚举属性
- [ ] `Object.assign()` / `Object.create()` — 属性复制 / 原型指定创建
- [ ] `Object.getPrototypeOf()` / `Object.setPrototypeOf()` — 原型链读取 / 设置
- [ ] 其他常用 Object 静态/原型方法

### 块 H：新值类型

无互依赖，可并行执行。

- [ ] **BigInt** — 新增 NaN-boxing tag（复用 tag bits 的空闲编码），解析器识别 `123n` 后缀，运行时实现 BigInt 算术（通过宿主函数）。影响层：IR（value.rs）+ 解析器 + 运行时
- [ ] **Symbol** — 新增 NaN-boxing tag，支持 `Symbol()` 和 `Symbol.for()`/`Symbol.keyFor()`，`Symbol.hasInstance` 自定义 instanceof 行为。影响层：IR + 语义 + 运行时

### 块 I：大型独立特性

无互依赖，可并行执行。

- [ ] **正则表达式 `/.../`** — 解析器识别 `RegExpLiteral`，运行时集成正则引擎（如 `regex` crate），`String.prototype.match`/`replace`/`search`/`split`。影响层：解析器 + 运行时
- [ ] **模块系统 — ES `import` / `export`** — 解析器识别 `ImportDecl`/`ExportDecl`，语义层构建模块依赖图，运行时实现模块加载和绑定。影响层：解析器 + 语义 + 运行时
- [ ] **模块系统 — CommonJS `require` / `module.exports`** — 运行时 `require` 函数 + `module` 对象。影响层：运行时
- [ ] **模块系统 — 动态 `import()`** — 异步模块加载，返回 Promise。影响层：语义 + 运行时
- [ ] **JSX** — 解析器识别 JSX 语法（需 swc `jsx` feature），语义层 lowering 为 `createElement` 调用。影响层：解析器 + 语义
- [ ] **TypeScript `interface` / `type` / `enum` / `namespace`** — TypeScript 特有声明，语义层解析类型并擦除（或保留在 IR 中以支持运行时反射）。影响层：解析器 + 语义
- [ ] **Proxy / Reflect** — 运行时实现 `Proxy` 构造函数和全部 handler trap（`get`/`set`/`has`/`deleteProperty`/`apply`/`construct` 等），`Reflect` 静态方法。影响层：运行时
- [ ] **`using` 声明（Explicit Resource Management）** — ES2024 Stage 3，`using x = expr` 在作用域退出时自动调用 `Symbol.dispose`。影响层：语义 + WASM 后端

### 块 J：动态执行

无外部依赖。

- [ ] **`eval` 函数** — 运行时动态编译执行 JavaScript 代码字符串，需要将整个编译管线（parse → lower → compile → execute）嵌入运行时。**test262 大量使用，优先级高。** 影响层：运行时

### 块 K：JIT 后端

无外部依赖。

- [ ] **`wjsm-backend-jit` 完整实现** — 当前为 stub，直接 `bail!("not implemented")`。需设计和实现 JIT 编译管线。影响层：`wjsm-backend-jit` crate
