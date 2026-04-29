# wjsm 缺失的基础语法（按实现优先级排序）

## 一、函数与类（完全缺失）← 当前最高优先级

- **函数声明** `function f() {}`
- **函数表达式** / **箭头函数** `const f = () => {}`
- **类声明** `class C {}`
- **`new` 表达式**
- **一般函数调用** — 仅 `console.log()` 被特判支持，其他如 `foo()` 直接报错

---

## 二、运算符（部分缺失）

已实现：`+`、`-`、`*`、`/`、`%`、`==`、`!=`、`===`、`!==`、`<`、`<=`、`>`、`>=`、`&&`、`||`、`??`、`!`、三元 `a ? b : c`、复合赋值（`+=`、`-=`、`*=`、`/=`、`%=`）

缺失：

| 类别 | 运算符 |
|------|--------|
| 位运算 | `|`、`^`、`&`、`<<`、`>>`、`>>>` |
| 幂运算 | `**`（IR/语义层已实现为 `F64Exp`，待确认后端完整支持） |
| 一元 | `~`、`-`(负)、`+`(正)、`typeof`、`void`、`delete`（`~` 语义层已实现但 backend bail；`-`/`+` 已实现；`void` 已实现） |
| 自增自减 | `++x`、`x++`、`--x`、`x--` |
| 逗号 | `a, b` |
| 其他 | `in`、`instanceof` |

复合赋值中 `**=`、`>>=`、`<<=`、`>>>=`、`|=`、`&=`、`^=`、`&&=`、`||=`、`??=` 也缺失。

---

## 三、字面量与常量（部分缺失）

已实现：数字、字符串、`true`/`false`（布尔字面量）、`null`、`undefined`

缺失：

- `BigInt`
- 正则表达式 `/.../`
- 模板字符串 `` `hello ${world}` ``
- 数组字面量 `[1, 2, 3]`
- 对象字面量 `{ a: 1 }`
- JSX

---

## 四、运行时基础设施

- 堆分配器 / GC
- 闭包（词法捕获 + 堆分配）
- `this` 绑定规则
- 除 `console.log` 外的所有宿主 API

---

## 五、对象与数据结构（完全缺失）

- 属性访问 `obj.prop`、`obj["prop"]`
- 可选链 `a?.b`
- `this`、`super`
- 对象 / 数组 / 原型链语义
- Proxy / Reflect

---

## 六、模块系统（完全缺失）

- `import` / `export`
- CommonJS `require` / `module.exports`
- 动态 `import()`

---

## 七、其他声明类型

- TypeScript 特有：`interface`、`type`、`enum`、`module`（命名空间）
- `using` 声明（Explicit Resource Management）
- 解构声明 `let {a, b} = obj`、`let [x, y] = arr`

---

## 八、控制流（大部分已实现）— 已完成，仅供参考

| 类别 | 特性 | 状态 |
|------|------|------|
| 条件 | `if`/`else` | ✅ 完整实现 |
| 条件 | `switch`（含 default fallthrough、case 内嵌 if/while） | ✅ 完整实现 |
| 循环 | `while`、`do...while`、`for` | ✅ 完整实现 |
| 循环 | `for...in`、`for...of` | ✅ 基础实现（仅支持 Ident LHS，解构 LHS 待 object/array 支持） |
| 跳转 | `break`、`continue`（含 label） | ✅ 完整实现 |
| 跳转 | `return`（含 finally） | ✅ 完整实现 |
| 跳转 | `labeled` 语句 | ✅ 完整实现 |
| 异常 | `try`/`catch`/`finally`、`throw` | ✅ 完整实现 |
| 其他 | `debugger`、空语句 | ✅ 完整实现（no-op） |
| 其他 | `with` | ❌ 明确不支持（废弃特性） |

---

## 九、JIT 后端

整个 `wjsm-backend-jit` 是一个 stub，直接 `bail!("JIT backend is not implemented yet")`。
