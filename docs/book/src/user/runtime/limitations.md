# 限制与已知差异

这些是 wjsm 当前版本（`0.1.0`）与 ECMAScript 规范或 Node.js 行为之间的已知差异。大部分源于 Direct Cranelift 架构的编译策略：builtin 方法在 IR 中以调用形态存在，不作为可读属性暴露。遇到不确定的行为时，跑一遍比查表快：

```bash
wjsm run -e 'console.log(typeof [].map)'
```

## Builtin 方法拦截

wjsm 在语义层拦截内置方法调用，生成专用的 `CallBuiltin` 指令；宿主同时在属性读取路径为这些方法合成可调用的函数值。大多数内建原型方法既可直接调用，也可取值后经 `call` / `apply` / `bind` 传递：

```js
typeof [].map        // "function"
typeof "abc".slice   // "function"
typeof fetch         // "function"
```

仍有少数名字只在调用点可用，取值得到 `undefined`，下文逐条说明。

## String 原型方法

`String.prototype` 是真实固有原型对象：已实现的方法（含 `match`、`matchAll`、`replace`、`split`、`trim`、locale 相关方法等）都是原型上的不可枚举数据属性，可取值后经 `call` / `apply` / `bind` 传递，`String.raw` 也已实现：

```js
"hello".match(/l/)                          // 可用：直接调用
const fn = "hello".match                    // function：取值成功
fn.call("world", /o/)[0]                    // "o"
String.prototype.slice.call("abcdef", 1, 4) // "bcd"
String.raw`a\nb${1}`                        // "a\\nb1"
```

未实现的名字不占位：Annex B 的 HTML 方法族（`anchor`、`big` 等）、`substr`、`isWellFormed` / `toWellFormed`、`trimLeft` / `trimRight` 在原型上不存在，读取得到 `undefined`。

## TypedArray / DataView

TypedArray 与 DataView 的原型方法可取值并经 `call` / `apply` / `bind` 复用，`name` / `length` 元数据、`Reflect.get` 与解构取值一致；各构造器的 `prototype` 对象（`Uint8Array.prototype.slice`、`DataView.prototype.getUint8` 等）同样可用。DataView 的 get/set 家族包含 `getBigInt64` / `getBigUint64` / `setBigInt64` / `setBigUint64`。

TypedArray 实例的原型链按 §23.2 完整挂接：实例 → `Constructor.prototype` → `%TypedArray%.prototype` → `%Object.prototype%`，`instanceof` 与 `Object.getPrototypeOf` 沿链成立；`length` / `byteLength` / `byteOffset` 是 `%TypedArray%.prototype` 上的规范 accessor（getter 名为 `get length` 等，可跨元素类型经 `call` 复用，品牌检查失败按 V8 口径抛 TypeError），`Constructor.prototype` 与构造器自身携带 `BYTES_PER_ELEMENT`。

```js
const buf = new Uint8Array([1, 2, 3]);
Object.getPrototypeOf(buf) === Uint8Array.prototype;   // true
buf instanceof Uint8Array;                             // true
const shared = Object.getPrototypeOf(Uint8Array.prototype); // %TypedArray%.prototype
Object.getOwnPropertyDescriptor(shared, "length").get.name;  // "get length"
Uint8Array.prototype.slice.call(buf, 0, 1);            // Uint8Array(1) [1]（沿链继承）
```

已知差异：`%TypedArray%` 抽象构造器本身不存在——`Object.getPrototypeOf(Uint8Array)` 不是 `TypedArray` 函数，`%TypedArray%.prototype` 上无自有 `buffer` 访问器；`map` / `filter` 返回普通数组而非同类型 TypedArray；DataView 实例的原型链仍未挂接到 `DataView.prototype`。

## Node Buffer 原型链

`Buffer.prototype` 按 Node 形态物化：own `constructor` 与已实现的实例方法是真实数据属性（可写可枚举可配置，Node 定义次序），`[[Prototype]]` 挂 `Uint8Array.prototype`，实例创建即接线三层链——覆盖 / 删除原型方法对实例立即可见，删除不复活。构造器静态链挂 `Uint8Array`（`Object.getPrototypeOf(Buffer) === Uint8Array`），`BYTES_PER_ELEMENT`、`of`、`@@species` 沿静态链继承：

```js
const buf = Buffer.from('ab');
Object.getPrototypeOf(buf) === Buffer.prototype;                    // true
Object.getPrototypeOf(Buffer.prototype) === Uint8Array.prototype;   // true
buf instanceof Uint8Array;                                          // true
buf instanceof Buffer;                                              // true
Uint8Array.prototype.slice.call(buf, 0, 1) instanceof Uint8Array;   // true（沿链继承）
```

已知差异：未实现的静态成员（`poolSize`、`copyBytesFrom`、`allocUnsafeSlow`、`compare`、`isEncoding`）与原型方法（BigInt 读写族、`readUIntLE` 变长族、小写 `Uint` 别名、`swap16/32/64`、`lastIndexOf`、`toLocaleString`、`inspect`、`parent` / `offset` 访问器）不占位；`Buffer` 的方法是宿主可调用值，无 Node 普通函数的自有 `prototype`；`Object.getOwnPropertyNames(Buffer)` 的静态成员次序与 Node 不同；`Buffer[Symbol.species]` 经静态链取到 `Buffer` 本身（Node 返回内部 `FastBuffer`）。

## TDZ 混合判定

`let` / `const` / `class` 的 Temporal Dead Zone 按引用位置分两种方式处理：

- **同函数内的前向引用**：执行时必然违规，lowering 期直接拒绝
  （`const set = { value: set }`、`{ console.log(x); let x = 1 }`）。
  `const` 重赋值同样在编译期报错。
- **跨函数前向引用**：函数体读取/写入后声明的 binding（[#372](https://github.com/juzi5201314/wjsm/issues/372)），
  静态无法判定调用是否先于声明执行，按规范降级为运行时检查——声明执行前
  访问抛 `ReferenceError: Cannot access 'x' before initialization`，之后正常读写。

```js
function early() { return x; }
try { early(); } catch (e) { console.log(e.name); } // "ReferenceError"
let x = 1;
console.log(early()); // 1

const set = {
  forEach(action) {
    action(set); // 初始化完成后调用：读取真实 binding
  },
};
set.forEach((value) => console.log(typeof value)); // "object"
```

类名同样适用：`class C { m() { return C.name } }` 的方法体、
`function f() { return new C(); } class C {}` 的前向构造都按运行时 TDZ 处理；
静态字段初始值、`extends` 等类定义期求值位置属于同函数直线执行，保持编译期拒绝。

## Intl 与 locale 敏感方法

`Intl` 命名空间、ECMA-402 构造器与 `String`/`Number`/`BigInt`/`Date`/`Array` 的 locale 敏感方法已实现，数据来自 `wjsm-intl-data`（ICU4X compiled_data）。默认 locale 读 `LC_ALL`/`LANG`，非法则 `en`。fixture 与可复现测试应显式传 locale。

实现定义差异（ILD/ILND）按规范允许，不把 Node full-icu 当作逐字符 oracle。Temporal 的 intl402 测试不在当前范围内。`intl-normative-optional` 的 legacy constructor 未实现。`localeMatcher: "best fit"` 当前与 Lookup 相同（规范允许实现定义）。`Intl.DisplayNames` 的 `calendar` / `dateTimeField` 目前只有英文表，其他 locale 回退为 code。`timeZoneName` 用 GMT 偏移与城市名，不是完整 ICU zone fieldset。

## URL / URLSearchParams 全局与 IDN

`URL` / `URLSearchParams` 可作为全局使用，也与 `import { URL } from "node:url"` 共享同一构造器。域名 hostname 走 UTS #46（`wjsm-intl-data` / `idna`），例如 `new URL("https://例え.テスト/")` 的 hostname 为 punycode。IPv4 / IPv6 字面量不跑 IDNA；`http://[::1]:8080/` 的 hostname 为 `::1`，host / href 带方括号。

```js
console.log(typeof globalThis.URL); // "function"
console.log(new URL("https://例子.测试/").hostname);
```
## --format native-executable 只覆盖当前宿主

`wjsm build --format native-executable` 在当前宿主上产出可直接运行的 ELF/PE：预链 `wjsm-exec` stub 加上 portable `.wjsm`、预编译 `NativeObject` 与制品内源码快照。Linux 上的 wjsm 出 ELF，Windows 上的 wjsm 出 PE。交叉编译、把 runtime-private object 改后缀冒充 executable，都不支持。打包失败不创建或覆盖输出文件。发行物需要同时带 `wjsm` 与 `wjsm-exec`。

packed exe 的源码 owner 是快照，不是主机目录。静态 `new Worker('./x')` / `fork('./x')` 会在打包期自动纳入；计算出来的入口仍须 `--include`。快照外模块与虚拟路径写操作明确失败。`wjsm run` 与 portable `.wjsm` 仍用主机路径。详见 [ADR 0017](../../../../adr/0017-native-executable-source-snapshot.md) 与 [ADR 0019](../../../../adr/0019-native-executable-application-contract.md)。

## 深入了解

- [语言功能矩阵](../reference/language-matrix.md)
- [Node.js 兼容矩阵](../reference/node-compatibility-matrix.md)
- [JavaScript 与 TypeScript 支持](javascript-and-typescript.md)
