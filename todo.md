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

## 二、运算符 — ✅ **全部实现**

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
- [x] **`~` 按位非**（UnaryOp::BitNot，ToInt32 后 XOR -1）
- [x] **位运算** `|`、`^`、`&`、`<<`、`>>`、`>>>`（BinaryOp，i32 操作）
- [x] **`typeof`**（Builtin::TypeOf，返回类型字符串）
- [x] **自增自减** `++x`、`x++`、`--x`、`x--`（UpdateExpr，LoadVar + Add/Sub + StoreVar）
- [x] **`in`**（Builtin::In，遍历原型链检查属性存在性）
- [x] **`instanceof`**（Builtin::InstanceOf，原型链检查 — 遍历 `__proto__` 链匹配 constructor.prototype）
- [x] **`delete`**（MemberExpr → DeleteProp，Ident → Const(true)）
- [x] **复合赋值扩展** `%=`、`**=`、`>>=`、`<<=`、`>>>=`、`|=`、`&=`、`^=`
- [x] **逻辑复合赋值** `&&=`、`||=`、`??=`（含短路求值）


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


---

## 十、复杂任务块

将第 10 节的全体已知限制与待完善按功能域划分为 5 个独立 Plan，按依赖关系依次执行。

**执行顺序**: `10.3 → 10.4 → 10.1 / 10.2 / 10.5`（后三者互无依赖，顺序自由）

---

### 10.1 闭包与词法作用域

**影响层**: IR + 语义 + WASM 后端 + 运行时
**估算**: 🔴 大（架构级别）
**内存策略**: 闭包环境只分配不释放，PoC 阶段可接受

- [ ] **闭包（词法变量捕获）完全不支持** — `push_function_context`/`pop_function_context` 完全更换作用域树，嵌套函数/箭头函数无法访问父作用域的变量。需要新增 `CreateClosure`/`LoadCaptured`/`StoreCaptured` IR 指令、逃逸分析、环境堆分配
- [ ] **箭头函数不捕获词法 `this`** — 当前 `this` 仅作为普通参数 `$this` 传递，统一在闭包机制中解决（`$this` 作为需要词法捕获的变量）

---

### 10.2 运算符隐式类型转换

**影响层**: WASM 后端 + 运行时
**估算**: 🟡 中

新建运行时宿主函数：`AbstractEq`、`CompareString`、`ToPrimitive`（验证已有 `string_concat`）

- [ ] **`+` 运算符字符串连接** — 验证并完善已有的 `string_concat` 两阶段逻辑（先尝试字符串连接，失败回退 F64Add），确保字符串 + 字符串、字符串 + 数字等组合正确
- [ ] **`==` 运算符缺少跨类型隐式转换** — 当前只实现了 `null == undefined` 特判和 `I64Eq`/`F64Eq`。需要实现完整 `AbstractEq`：`ToPrimitive`、`ToNumber`、`ToBoolean` 跨类型转换
- [ ] **`<`/`>`/`<=`/`>=` 只做 f64 数值比较** — 缺少字符串字典序比较、`null`/`undefined` 特殊数值转换、对象的 `ToPrimitive`
- [ ] **`for...in` 枚举器在运行时只对字符串有实际实现** — `enumerator_from` 对非字符串值仅 push `Error` 状态

> 📌 一元 `+`（UnaryOp::Pos）和 `+` 运算符已有部分实现（`string_concat` 尝试、`ToNumber` 转换），10.2 中验证并修复即可

---

### 10.3 对象系统与属性描述符

**影响层**: IR + WASM 后端 + 运行时
**估算**: 🟠 中大
**核心设计**: 属性存储格式从 `[name_id, value]` 扩展为 `[name_id, value, flags]`，flags 包含 configurable/writable/enumerable 各 1 bit

- [ ] **对象属性存储格式重设计** — 新增 flags 元数据槽位，支持每个属性的 configurable/writable/enumerable 标志
- [ ] **对象初始容量固定为 4，超出后静默丢弃属性** — `$obj_set` 检查 `num_props < capacity` 但无扩容逻辑，满容量时属性静默丢失。需要实现扩容（超出 capacity 时重新分配更大的内存）
- [ ] **`$obj_delete` 的 configurable 检查和严格模式检查** — 当前始终返回 true/false 仅基于属性存在性，未检查 configurable 标志
- [ ] **`Object.defineProperty()` / `Object.getOwnPropertyDescriptor()`** — runtime 宿主函数，操作属性描述符
- [ ] **`SetProto` 不做有效性验证** — 直接向对象内存 offset 0 写入原始 i32 指针，不检查是否为有效对象指针
- [ ] **函数注册时 `func_props` 使用固定容量** — 每个函数对应 8 字节固定槽位，缺少按需分配

---

### 10.4 Object 方法

**依赖**: 10.3（需要对象系统基础）
**影响层**: IR + 语义 + WASM 后端 + 运行时
**估算**: 🟢 中小
**实现路径**: Builtin 指令方案（与现有 `console.log` 模式一致），新增 `Builtin::HasOwnProperty` 等 IR 指令，语义层识别对应方法名模式

- [ ] `Object.prototype.hasOwnProperty()`
- [ ] `Object.keys()` / `Object.values()` / `Object.entries()`
- [ ] `Object.assign()` / `Object.create()`
- [ ] `Object.getPrototypeOf()` / `Object.setPrototypeOf()`
- [ ] 其他常用 Object 静态/原型方法

---

### 10.5 函数·类·语法特性

**影响层**: IR + 语义 + WASM 后端 + 运行时
**估算**: 🟠 中大

- [ ] **函数调用最多支持 7 个普通参数** — WASM 函数类型限制，`args.iter().take(7)` 静默丢弃超量参数。需要扩展调用约定
- [ ] **函数参数不支持解构和默认值** — 参数匹配只处理 `Pat::Ident`，其他模式（解构、默认值）被 `filter_map` 静默忽略
- [ ] **类不支持 getter/setter 方法** — `lower_class_decl` 只处理 `MethodKind::Method`，跳过 getter/setter
- [ ] **类不支持静态方法和静态块** — 所有方法都放在 prototype 上，`StaticBlock` 等未处理
- [ ] **`new` 表达式中 prototype 查找不完整** — 仅在函数自身的属性对象上搜索 `prototype`，不走完整 `[[Get]]`（不会遍历函数原型链）
- [ ] **`this` 绑定规则不完整** — 只实现了 `obj.method()` 模式；缺少 `func.call()`/`apply()`/`bind()`、`method()`（非严格模式下 this → global/undefined）等
- [ ] **对象字面量不支持计算属性名和 spread** — `lower_object_expr` 只接受 `PropName::Ident`/`PropName::Str`，计算属性和 spread 报错
- [ ] **Symbol 支持** — `Symbol.hasInstance`（自定义 instanceof 行为）+ 其他 Symbol 属性

---

### 已确认的设计边界（非 Plan 目标）

以下限制已在语义层显式捕获并报错，当前不纳入任何 Plan 的修复范围：

- `++x`/`x++`/`--x`/`x--` 只支持标识符操作数（`obj.x++`、`arr[i]++` 等成员表达式操作数不支持）
- `obj.x += 1` 等复合赋值到成员表达式不支持（语义层显式返回错误）