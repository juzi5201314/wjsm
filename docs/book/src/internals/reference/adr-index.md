# ADR 导航

| ADR | 标题 | 状态 |
| --- | --- | --- |
| 0001–0009 | Symbol、RuntimeState、snapshot、inspector、realm、async hooks 等早期决策 | 按各文件状态；0003 于 2026-08-17 改为强制 restore |
| 0010 | Generational ZGC Managed Heap | Accepted，当前 GC 基线 |
| 0011 | Runtime Split by Backend Independence | 历史；Wasmtime 生产路径由 0014 取代 |
| 0012 | Host Builtins Decouple | Accepted；后端无关 builtins/host 分层继续有效 |
| 0013 | Multi Backend Contract | Superseded by 0014 |
| 0014 | Direct Cranelift 与 portable `.wjsm` 终态 | Accepted，当前架构基线 |
| 0015 | Builtin 段 native 镜像复用 | Accepted |
| 0016 | 同宿主 native executable 为 stub + overlay | Accepted；修正 0014 §6 |
| 0017 | native-executable 以制品内源码快照为运行时源码 owner | Accepted；修正 0016 §2 |
| 0018 | native-executable overlay payload 整层 zstd | Accepted；修正 0016 §1 |
| 0019 | native-executable 的 packed 应用合同 | Accepted；修正 0016 §5 与 0017 |
| 0020 | ICU4X compiled_data 嵌入 rustc 链接的 stub | Accepted；Intl Phase 1 数据契约，Phase 2/3 已消费 |
| 0021 | Latin-1 inline string | Accepted |
| 0022 | 投机 typed 区、deopt 与 OSR | Accepted；修正 0014 overlay 合同。AOT 只覆盖 generic native 时机，overlay 是运行时特化 |

## 当前基线

Direct production chain 是 verified semantic IR → canonical portable `.wjsm` → direct IR→CLIF → current-host generic native image → `NativeRuntime`。`.wjsm` 是唯一跨平台用户制品（IR，不含机器码）；native cache、snapshot 与 image 是可重建的 runtime-private 派生数据。语言仍是动态 JS；overlay / `eval` 在运行时调用同一编译器，不是第二套执行后端。

`wjsm build --format native-executable` 产出同宿主 stub+overlay ELF/PE（ADR 0016），运行时源码 owner 是制品内快照（ADR 0017），overlay 正文整层 zstd（ADR 0018），packed 应用合同见 ADR 0019。CLDR/Unicode 数据由 ICU4X compiled_data 嵌入 rustc 链接的 `wjsm` / `wjsm-exec` stub（ADR 0020），不进 `.wjsm` 或 startup snapshot。不支持的平台 fail-closed，不切换到 Wasm、解释器或另一套执行引擎。

## 参考

- [ADR 0010](../../../../adr/0010-generational-zgc-managed-heap.md)
- [ADR 0014](../../../../adr/0014-direct-cranelift-portable-artifact.md)
- [ADR 0020](../../../../adr/0020-icu4x-baked-data.md)
- [ADR 0022](../../../../adr/0022-speculative-typed-regions.md)
- [Direct Cranelift 后端](../backend/README.md)
- [Owner 与单一事实来源](owners-and-sources-of-truth.md)
