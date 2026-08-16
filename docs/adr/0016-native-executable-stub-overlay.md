# ADR 0016: 同宿主 native executable 为 stub + overlay

## Status

Accepted（2026-08-16）

Amends ADR 0014 §6。不改变 `.wjsm` 作为唯一跨平台用户制品、`NativeRuntime` 作为唯一运行时 owner、或 Cranelift/object/platform 依赖边界。

## Context

ADR 0014 把 `wjsm build --format native-executable` 排除在范围外，并禁止把 runtime 私有 relocatable object 伪装成可执行文件。用户随后要求得到与 `cargo build` 同类的同宿主 ELF/PE，且 `wjsm build` 不调用 `ld` / `lld` / `link.exe`，也不引入通用第三方 linker。

Guest 侧已经具备受限 linker：`CompiledImage::load` 只接受 Cranelift object、闭集 reloc 与 `NativeHostSymbol` allowlist。缺的是把 `NativeRuntime` 装进 OS 进程。Host 侧（std、GC、I/O、rustls）必须由 rustc 在构建 wjsm 时链一次；自研通用 linker 去消费 rustc rlib 等于重写 lld。

## Decision

### 1. native-executable 是同宿主 stub + overlay

`wjsm build --format native-executable` 产出真实平台可执行文件：

```text
[预链 wjsm-exec stub（rustc 在构建 wjsm 时链接）]
[payload：PortableArtifact + 1～2 个 NativeObject + ABI/codegen/target 元数据]
[footer：WJSMEXEC magic、version、offset、length、digest]
```

文件是 rustc 已经链好的 ELF/PE，不是把 `.o` 改后缀。overlay 不得改写 PE/ELF 头。装载仍只走 `CompiledImage::load`。

### 2. 构建时预编译，启动跳过 codegen

payload 同时携带 canonical `.wjsm` 与预编译 `NativeObject`。有 `$builtin_main` 时按 ADR 0015 切成两段 object，与 `NativeRuntime::execute` 一致。启动用预编译 object 发布 image，不走 `NativeCompiler::compile`。`.wjsm` 仍用于 manifest、`install_program`、eval 与 worker；丢掉 IR 会变成残缺语义。

### 3. 系统 linker 只出现在构建 wjsm 时

`wjsm` 与 `wjsm-exec` 由 rustc 链接。用户执行 `wjsm build --format native-executable` 时只复制 stub、编码 payload、写 footer。禁止 `Command` 调用平台 linker，禁止引入 wild / lld-rs 等通用 linker crate，禁止自研消费 rustc rlib 的通用 linker。

### 4. 同宿主、fail-closed

`NativeCompiler` 仍只编当前宿主 ISA。Linux 上的 wjsm 出 ELF，Windows 上的 wjsm 出 PE。交叉编译不是本决策范围。payload 的 native ABI hash、codegen hash、target 与 Cranelift 版本必须与 stub 一致，否则拒绝执行。打包失败不创建或覆盖输出文件。

### 5. 第一刀关闭特化 overlay

`wjsm-exec` 以 `specialization_enabled = false` 启动，与 `WJSM_DISABLE_SPECIALIZATION=1` 同语义。generic AOT、IC 与 eval 路径保持完整；eval 仍可走 stub 内的 compiler。

### 6. Owner

| 职责 | Owner |
| --- | --- |
| footer / payload 编解码 | `wjsm-exec-format`（无 Cranelift） |
| 预编译 object 与 `CompiledImage::load` | `wjsm-backend-native` |
| `execute_precompiled` / 打包用 compile | `wjsm-host-native` |
| stub 进程入口 | `wjsm-exec` |
| CLI 打包与失败不覆盖 | `wjsm-cli` |

`.wjsm` 仍是唯一可跨平台携带的用户制品。native-executable 是当前宿主派生的分发形态，不可当作 portable artifact。

## Consequences

- ADR 0014 §6 的 NotImplemented 合同退役；runtime-private object 仍不得单独充当 executable。
- 发行物需同时提供 `wjsm` 与 `wjsm-exec`。
- 用户可执行文件体积接近去掉 CLI 的宿主 runtime。
- 把 guest `.text` 合进 stub PT_LOAD、或对 `libwjsm` 写导入表，须另立 ADR；现有 load-time reloc 已足够。

## Verification

- payload 编解码、footer 损坏、ABI/target 失配测试。
- CLI：失败不覆盖已有目标；成功则 `build -e 'console.log(1)'` 产出可执行文件且 stdout 为 `1`。
- `execute_precompiled` 与 compile 路径对同一 artifact 的可观察结果一致。
- 文档与 ADR 0014 §6 同步为 stub+overlay 合同。

## References

- ADR 0014 — Direct Cranelift 与 portable `.wjsm` 终态
- ADR 0015 — Builtin 段 native 镜像复用
- `docs/backend-implementation-guide.md`
