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

## 十、已知限制与待完善

### 属性描述符系统
- [ ] `configurable` / `writable` / `enumerable` 标志
- [ ] `$obj_delete` 的 configurable 检查和严格模式检查（当前始终返回 true/false 仅基于属性存在性）
- [ ] `Object.defineProperty()` / `Object.getOwnPropertyDescriptor()`

### Object 方法
- [ ] `Object.prototype.hasOwnProperty()`
- [ ] `Object.keys()` / `Object.values()` / `Object.entries()`
- [ ] `Object.assign()` / `Object.create()`
- [ ] `Object.getPrototypeOf()` / `Object.setPrototypeOf()`
- [ ] 其他 Object 静态/原型方法

### Symbol 支持
- [ ] `Symbol.hasInstance`（自定义 instanceof 行为）
- [ ] 其他 Symbol 属性

### 函数与类实现简化
- [ ] **箭头函数不捕获词法 `this`** — 当前 `this` 仅作为普通参数 `$this` 传递，箭头函数的 `this` 应当从定义时的外层作用域词法继承
- [ ] **闭包（词法变量捕获）完全不支持** — `push_function_context`/`pop_function_context` 完全更换作用域树，嵌套函数/箭头函数无法访问父作用域的变量
- [ ] **函数调用最多支持 7 个普通参数** — WASM Type 6 签名只有 8 个 i64 槽（this + 7 参数），`args.iter().take(7)` 静默丢弃超量参数
- [ ] **函数参数不支持解构和默认值** — 参数匹配只处理 `Pat::Ident`，其他模式（解构、默认值）被 `filter_map` 静默忽略
- [ ] **类不支持 getter/setter 方法** — `lower_class_decl` 只处理 `MethodKind::Method`，跳过 getter/setter
- [ ] **类不支持静态方法和静态块** — 所有方法都放在 prototype 上，`StaticBlock` 等未处理
- [ ] **`new` 表达式中 prototype 查找不完整** — 仅在函数自身的属性对象上搜索 `prototype`，不走完整 `[[Get]]`（不会遍历函数原型链）
- [ ] **`this` 绑定规则不完整** — 只实现了 `obj.method()` 模式；缺少 `func.call()`/`apply()`/`bind()`、`method()`（非严格模式下 this → global/undefined）等

### 运算符实现简化
- [ ] **`+` 运算符只做数值加法，不支持字符串连接** — WASM 后端直接用 `F64ReinterpretI64 → F64Add`，字符串相加产生垃圾结果
- [ ] **一元 `+`（UnaryOp::Pos）是空操作（no-op）** — 按 JS 规范应执行 `ToNumber(x)`，但当前仅复制值
- [ ] **`==` 运算符只实现了 `null == undefined` 特判** — 缺少其他类型间的隐式转换（如 `"1" == 1`、`[1] == 1` 等）
- [ ] **`<`/`>`/`<=`/`>=` 只做 f64 数值比较** — 缺少字符串字典序比较、`null`/`undefined` 特殊数值转换、`ToPrimitive` 等
- [ ] **`++x`/`x++`/`--x`/`x--` 只支持标识符操作数** — `obj.x++`、`arr[i]++` 等成员表达式操作数不支持
- [ ] **`obj.x += 1` 等复合赋值到成员表达式不支持** — 语义层显式返回错误
- [ ] **对象字面量不支持计算属性名和 spread** — `lower_object_expr` 只接受 `PropName::Ident`/`PropName::Str`，计算属性和 spread 报错
- [ ] **`for...in` 枚举器在运行时只对字符串有实际实现** — `enumerator_from` 对非字符串值仅 push `Error` 状态

### 运行时对象系统简化
- [ ] **对象初始容量固定为 4，超出后静默丢弃属性** — `$obj_set` 检查 `num_props < capacity` 但无扩容逻辑，满容量时属性静默丢失
- [ ] **`SetProto` 不做有效性验证** — 直接向对象内存 offset 0 写入原始 i32 指针，不检查是否为有效对象指针
- [ ] **函数注册时 `func_props` 使用固定容量** — 每个函数对应 8 字节固定槽位，缺少按需分配