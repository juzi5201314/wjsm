# wjsm 缺失的基础语法

## 一、控制流（完全缺失）

| 类别 | 缺失特性 |
|------|---------|
| 条件 | `if`/`else`、`switch` |
| 循环 | `while`、`do...while`、`for`、`for...in`、`for...of` |
| 跳转 | `break`、`continue`、`return`、`labeled` 语句 |
| 异常 | `try`/`catch`/`finally`、`throw` |
| 其他 | `with`、`debugger`、空语句 |

IR 层面没有分支/跳转指令（Branch、Jump、Switch、Phi、Unreachable 均未定义），也没有多基本块 CFG 支持。

## 二、函数与类（完全缺失）

- **函数声明** `function f() {}`
- **函数表达式** / **箭头函数** `const f = () => {}`
- **类声明** `class C {}`
- **`new` 表达式**
- **一般函数调用** — 仅 `console.log()` 被特判支持，其他如 `foo()` 直接报错

## 三、运算符（大部分缺失）

已实现：`+`、`-`、`*`、`/`

缺失：

| 类别 | 运算符 |
|------|--------|
| 比较 | `==`、`!=`、`===`、`!==`、`<`、`<=`、`>`、`>=` |
| 逻辑 | `\|\|`、`&&` |
| 位运算 | `\|`、`^`、`&`、`<<`、`>>`、`>>>` |
| 取模 | `%` |
| 幂运算 | `**` |
| 空值合并 | `??` |
| 一元 | `!`、`~`、`-`(负)、`+`(正)、`typeof`、`void`、`delete` |
| 自增自减 | `++x`、`x++`、`--x`、`x--` |
| 三元 | `a ? b : c` |
| 逗号 | `a, b` |
| 其他 | `in`、`instanceof` |

复合赋值中 `%=`、`**=`、`>>=`、`<<=`、`>>>=`、`\|=`、`&=`、`^=`、`&&=`、`\|\|=`、`??=` 也缺失。

## 四、字面量与常量（部分缺失）

已实现：数字、字符串

缺失：

- `true` / `false`（布尔字面量）
- `null`
- `BigInt`
- 正则表达式 `/.../`
- 模板字符串 `` `hello ${world}` ``
- 数组字面量 `[1, 2, 3]`
- 对象字面量 `{ a: 1 }`
- JSX

## 五、对象与数据结构（完全缺失）

- 属性访问 `obj.prop`、`obj["prop"]`
- 可选链 `a?.b`
- `this`、`super`
- 对象 / 数组 / 原型链语义
- Proxy / Reflect

## 六、模块系统（完全缺失）

- `import` / `export`
- CommonJS `require` / `module.exports`
- 动态 `import()`

## 七、其他声明类型

- TypeScript 特有：`interface`、`type`、`enum`、`module`（命名空间）
- `using` 声明（Explicit Resource Management）
- 解构声明 `let {a, b} = obj`、`let [x, y] = arr`

## 八、运行时基础设施

- 堆分配器 / GC
- 闭包（词法捕获 + 堆分配）
- `this` 绑定规则
- 除 `console.log` 外的所有宿主 API

## 九、JIT 后端

整个 `wjsm-backend-jit` 是一个 stub，直接 `bail!("JIT backend is not implemented yet")`。
