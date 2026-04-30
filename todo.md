# wjsm 缺失的基础语法（按实现优先级排序）

> ✅ = 已完成全链路（语义层 + WASM 后端 + 运行时）
> 🚧 = 语义层已实现，后端/运行时待完善
> ❌ = 未实现

---

## 一、函数与类 — ✅ **全部实现**

- [x] **函数声明** `function f() {}`
- [x] **函数表达式** / **箭头函数** `const f = () => {}`
- [x] **类声明** `class C {}`（含构造函数、方法、prototype 链）
- [x] **`new` 表达式**（创建对象、设置 `__proto__`、调用构造函数）
- [x] **一般函数调用**（含 `obj.method()` 形式的 this 绑定）
- [x] **类表达式** `class { ... }`（匿名类）

---

## 二、运算符

### 已实现：全部 ✅

- [x] `+`、`-`、`*`、`/`
- [x] `%`（通过 `env.f64_mod` host function）
- [x] `**`（通过 `env.f64_pow` host function）
- [x] `==`、`!=`（含 `null == undefined` 特判）
- [x] `===`、`!==`（混合 f64/NaN-boxed 严格比较）
- [x] `<`、`<=`、`>`、`>=`（f64 数值比较）
- [x] `&&`、`||`、`??`（短路的 CFG + Phi）
- [x] `!`（UnaryOp::Not）
- [x] `-`（一元负号，UnaryOp::Neg）
- [x] `+`（一元正号，UnaryOp::Pos）
- [x] `void`（UnaryOp::Void → undefined）
- [x] 三元 `a ? b : c`（CFG + Phi）
- [x] 逗号表达式 `a, b`
- [x] 复合赋值 `+=`、`-=`、`*=`、`/=`

### 语义层实现但后端 bail 🚧

- [ ] **`~` 按位非** — 语义层发出 `UnaryOp::BitNot`，后端 bail
- [ ] **位运算** `|`、`^`、`&`、`<<`、`>>`、`>>>` — 语义层发出 `CallBuiltin` 占位后 bail

### 完全缺失 ❌

- [ ] **`typeof`** — 语义层返回 `"typeof operator is not yet supported"`
- [ ] **自增自减** `++x`、`x++`、`--x`、`x--`
- [ ] **`in` / `instanceof`**
- [ ] **`delete`**
- [ ] **复合赋值**：`%=`、`**=`、`>>=`、`<<=`、`>>>=`、`|=`、`&=`、`^=`、`&&=`、`||=`、`??=`

---

## 三、字面量与常量

### 已实现：全部 ✅

- [x] 数字
- [x] 字符串
- [x] `true` / `false`（布尔字面量）
- [x] `null`
- [x] `undefined`
- [x] **对象字面量** `{ a: 1 }`（含 shorthand 属性）

### 完全缺失 ❌

- [ ] `BigInt`
- [ ] 正则表达式 `/.../`
- [ ] 模板字符串 `` `hello ${world}` ``
- [ ] 数组字面量 `[1, 2, 3]`
- [ ] JSX

---

## 四、运行时基础设施

### 已实现：部分

- [x] **数组对象堆分配** — `$obj_new` 在线性内存上分配对象
- [x] **原型链遍历** — `$obj_get` 沿 `__proto__` 链查找属性
- [x] **函数属性对象** — `func_props` 数组为每个函数保留属性对象
- [x] **NaN-boxed 值编码** — f64、string ptr、bool、null、undefined、object handle、function ref、exception、iterator、enumerator

### ❌ 缺失

- [ ] **完整堆分配器 / GC** — 当前仅简单 bump allocator，无法回收
- [ ] **闭包（词法捕获 + 堆分配环境）**
- [ ] **`this` 绑定完整规则** — 仅实现了 `obj.method()` 和箭头函数闭包捕获 `$this`
- [ ] **除 `console.log` 外的所有宿主 API**（`console.error`、`setTimeout`、`fetch` 等）

---

## 五、对象与数据结构

### 已实现：✅

- [x] 属性访问 `obj.prop`、`obj["prop"]`
- [x] 属性赋值 `obj.prop = val`
- [x] `new` 表达式（含 prototype 链接）
- [x] `this` 关键字（绑定到 `$this` 变量）
- [x] `prototype` 链（`SetProto` 指令 + `$obj_get` 原型遍历）

### 完全缺失 ❌

- [ ] 可选链 `a?.b`
- [ ] `super`
- [ ] 数组语义（`[1, 2, 3]`、`push`、`map` 等）
- [ ] Proxy / Reflect

---

## 六、模块系统 — ❌ 完全缺失

- [ ] `import` / `export`
- [ ] CommonJS `require` / `module.exports`
- [ ] 动态 `import()`

---

## 七、其他声明类型

### ❌ 全部缺失

- [ ] TypeScript `interface`、`type`、`enum`、`namespace`
- [ ] `using` 声明（Explicit Resource Management）
- [ ] 解构声明 `let {a, b} = obj`、`let [x, y] = arr`

---

## 八、控制流 — ✅ **全部实现**

| 类别 | 特性 | 状态 |
|------|------|------|
| 条件 | `if`/`else` | ✅ 完整实现 |
| 条件 | `switch`（含 default fallthrough、case 内嵌 if/while/switch） | ✅ 完整实现 |
| 循环 | `while`、`do...while`、`for` | ✅ 完整实现 |
| 循环 | `for...in`、`for...of` | ✅ 基础实现（仅支持 Ident LHS，解构 LHS 待 object/array 支持） |
| 跳转 | `break`、`continue`（含 label + 迭代器清理） | ✅ 完整实现 |
| 跳转 | `return`（含 finally → 从内到外展开） | ✅ 完整实现 |
| 跳转 | `labeled` 语句 | ✅ 完整实现 |
| 异常 | `try`/`catch`/`finally`、`throw` | ✅ 完整实现 |
| 其他 | `debugger`、空语句 | ✅ 完整实现（no-op） |
| 其他 | `with` | ❌ 明确不支持（废弃特性） |

---

## 九、JIT 后端 — 🚧 **stub**

- [ ] 整个 `wjsm-backend-jit` 是一个 stub，直接 `bail!("JIT backend is not implemented yet")`
