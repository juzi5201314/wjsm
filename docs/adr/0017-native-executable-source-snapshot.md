# ADR 0017: native-executable 以制品内源码快照为运行时源码 owner

## Status

Accepted（2026-08-16）

Amends ADR 0016 §2。不改变 stub+overlay、同宿主、不调用系统 linker、`.wjsm` 为唯一跨平台制品，或 `NativeRuntime` 唯一 owner。

## Context

ADR 0016 把打包期 `module_root` 写成主机绝对路径。静态已打进 `.wjsm` 的模块图可以跑，但 `import.meta.resolve`、动态 `import()` / `require()`、JSON 模块和 `new Worker` 仍按真实目录树解析并读盘。把 exe 拷到另一台同 OS/arch 机器后，这些路径失效。

运行时模块栈（`ModuleResolver`、`package.json`、JSON 加载、worker 冷编译）假定 `std::fs`。只改 stub 里的字符串路径不够。

## Decision

### 1. packed exe 的源码 owner 是 overlay 快照

`wjsm build --format native-executable` 把源文件打进 payload。启动后解析、加载、worker 编译只读这份快照。禁止回退 `cwd`、exe 旁目录或残留主机路径。

快照 = 打包期 store 实际读过的文件（模块、JSON、`package.json`）、静态可解析的 `new Worker('./x')` / `child_process.fork('./x')` / `cluster.setupMaster({ exec: './x' })` 相对路径，加上 `--include` 显式补入的文件。不默认打包整个 `--root`。`--include` 只属于 native-executable；越界或缺文件则打包失败且不写输出。

`/wjsm-exec/...` 与 `file:///wjsm-exec/...` 的 `node:fs` 读只走快照；写虚拟路径返回 EROFS。`cwd` 与其它主机路径仍走真盘。合同见 [ADR 0019](0019-native-executable-application-contract.md)。

### 2. 虚拟身份，不是构建机 `file://`

打包 lowering 把 `import.meta.filename` / `dirname` / `url` 与 CJS `__filename` 写成虚拟身份：

```text
/wjsm-exec/<logical_url>
file:///wjsm-exec/<logical_url>
```

`wjsm run` 与 portable `.wjsm` 仍用主机路径。

### 3. `ModuleSourceStore` 是唯一文件系统入口

`wjsm-module` 拥有 store：`Disk`（`wjsm run` / 打包期读盘）、`Recording`（打包期收集）、`Snapshot`（packed 运行时）。resolver、`package.json`、runtime resolution 不再直读 `std::fs`。未命中快照即 NotFound。

### 4. payload schema 3 删除主机 `module_root`

`PAYLOAD_SCHEMA` 升到 3。删除主机绝对路径字段，改为 `logical_url → bytes`。不双读 schema 2 的 `module_root`。

## Consequences

- packed exe 可在同 OS/arch 上单文件分发；动态加载仍用 stub 内 compiler 从快照源码编译。
- 主图未碰到的 worker 入口必须 `--include`。
- 发行物合同从「同机启动器」变成「可搬运单文件」，但仍不是跨平台制品。

## Verification

- 把 exe 拷走、源码树不可达，静态图与快照内 JSON / 动态 import / `--include` worker 仍成功。
- `import.meta.resolve` 前缀为 `file:///wjsm-exec/`，不含构建机绝对路径。
- 快照外模块 fail-closed；`wjsm run` 主机路径不变。

## References

- ADR 0016 — 同宿主 native executable 为 stub + overlay
- ADR 0006 — Runtime module loading boundary
- ADR 0019 — packed 应用合同
