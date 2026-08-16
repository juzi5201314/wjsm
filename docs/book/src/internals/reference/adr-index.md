# ADR 导航

| ADR | 标题 | 状态 |
| --- | --- | --- |
| 0001–0009 | Symbol、RuntimeState、snapshot、inspector、realm、async hooks 等早期决策 | 按各文件状态 |
| 0010 | Generational ZGC Managed Heap | Accepted，当前 GC 基线 |
| 0011 | Runtime Split by Backend Independence | 历史；Wasmtime 生产路径由 0014 取代 |
| 0012 | Host Builtins Decouple | Accepted；后端无关 builtins/host 分层继续有效 |
| 0013 | Multi Backend Contract | Superseded by 0014 |
| 0014 | Direct Cranelift 与 portable `.wjsm` 终态 | Accepted，当前架构基线 |
| 0015 | Builtin 段 native 镜像复用 | Accepted |
| 0016 | 同宿主 native executable 为 stub + overlay | Accepted；修正 0014 §6 |
| 0017 | native-executable 以制品内源码快照为运行时源码 owner | Accepted；修正 0016 §2 |
| 0018 | native-executable overlay payload 整层 zstd | Accepted；修正 0016 §1 |

## 当前基线

Direct production chain 是 verified semantic IR → canonical portable `.wjsm` → direct IR→CLIF → current-host native image → `NativeRuntime`。`.wjsm` 是唯一跨平台用户制品；native cache、snapshot 与 image 是可重建的 runtime-private 派生数据。

`wjsm build --format native-executable` 产出同宿主 stub+overlay ELF/PE（ADR 0016），运行时源码 owner 是制品内快照（ADR 0017），overlay 正文整层 zstd（ADR 0018）。不支持的平台 fail-closed，不切换到 Wasm/JIT/解释器。

## 参考

- [ADR 0010](../../../../adr/0010-generational-zgc-managed-heap.md)
- [ADR 0014](../../../../adr/0014-direct-cranelift-portable-artifact.md)
- [Direct Cranelift 后端](../backend/README.md)
- [Owner 与单一事实来源](owners-and-sources-of-truth.md)
