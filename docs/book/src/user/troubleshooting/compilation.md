# 编译与 WASM 问题

## 模块找不到

相对路径失败时报错会列出尝试过的候选：

```text
Failed to build module graph: Cannot find module './nope.js' from '/tmp/x/a.mjs'.
Tried: ["/tmp/x/./nope.js"]
```

裸包名失败则是：

```text
Failed to build module graph: Cannot find module 'no-such-pkg'
```

检查顺序：文件是否存在、扩展名是否在 `js`、`ts`、`mjs`、`cjs`、`jsx`、`tsx` 之内、包是否已 `wjsm install`、`--root` 是否覆盖了该文件所在目录。

## 未知的 node: 模块

```text
Failed to build module graph: Unknown built-in module 'node:not_real'
```

带 `node:` 前缀的说明符只接受已实现的模块名，清单见 [Node.js 兼容能力](../runtime/node-compatibility.md)。

## 写不出 .wasm

两种保护性拒绝：

- `-o -` 且 stdout 是终端时拒绝写出二进制。重定向到文件。
- `--stage parse` 或 `--stage lower` 配合 `-o` 时拒绝，因为这两个阶段输出文本到 stdout。

默认输出 `out.wasm`，若文件已存在会打印覆盖警告（`-q` 可抑制）。

## 校验失败

```bash
wjsm validate app.wasm
```

`magic header not detected` 说明文件不是 WASM；`unexpected end-of-file` 说明被截断。若产物是 wjsm 生成的却校验失败，说明写入过程中断，重新构建。

## 产物在别的运行时里跑不起来

wjsm 产物依赖 507 个宿主 import 和三块由宿主提供的内存，无法独立运行。细节见 [WASM 产物与宿主要求](../output/wasm-artifacts.md)。

## 深入了解

- [WASM 编译阶段](../../internals/pipeline/compile.md)
- [WASM 校验与尺寸分析](../../internals/tooling/validation-and-size.md)
