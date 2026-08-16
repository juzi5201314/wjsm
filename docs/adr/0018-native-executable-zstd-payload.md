# ADR 0018: native-executable overlay payload 整层 zstd

## Status

Accepted（2026-08-16）

Amends ADR 0016 §1。不改变 stub+overlay、同宿主、不改写 PE/ELF 头、不调用系统 linker、或 ADR 0017 的快照 owner。

## Context

packed exe 的体积大头是预链 `wjsm-exec` stub（发行约 27 MiB）。overlay 里的 `.wjsm`、NativeObject 与源码快照在宽图上仍有数兆到上百兆。schema 3 原样存放这些字节。

实测（zstd 级别 3）：

- 宽图 overlay 8.7 MiB → 0.73 MiB，压缩约 20 ms，解压约 9 ms
- `--include` 进 195 MiB 重复 blob → 29 KiB
- stub ELF 本身也能压到约 30%，但压缩后不再是可执行映像

启动路径已经整文件读入再 `unpack`。整层压缩不引入新的随机访问需求。

## Decision

### 1. payload schema 4：内层不变，外层 zstd

`PAYLOAD_SCHEMA` 升到 4。磁盘上的 overlay 正文为：

```text
u32 schema = 4
u64 raw_len
[zstd(inner)]
```

`inner` 仍是 ADR 0017 的字段：ABI/codegen hash、target 元数据、`logical_url → bytes` 快照、artifact、1～2 个 NativeObject。不双读 schema 3。

压缩级别固定为 3。`raw_len` 与压缩后长度都受现有 512 MiB payload 上限约束；解压输出必须恰好等于 `raw_len`。

### 2. 只压 overlay，不压 stub

stub 仍是 rustc 预链的 ELF/PE，overlay 不得改写其头。把 stub 本身 zstd 掉需要自解压 trampoline，另立决策。

### 3. Owner

编解码仍只属于 `wjsm-exec-format`。`wjsm-exec` 继续只调用 `unpack`。

## Consequences

- 宽图发行 exe 从 35.3 MiB 降到 27.4 MiB（overlay 8.7 → 0.70 MiB）。
- stub 因链入 libzstd 大约增加 150 KiB；`console.log(1)` 这类几乎无 overlay 的 exe 会略增。
- 启动多一次 zstd 解压；宽图约 10 ms，百兆快照也通常低于 1 s。
- 已有 schema 3 exe 不能再启动。制品可重建，不保留双读。
- 引入 `zstd` crate（C libzstd，关闭 default features）。Cranelift 边界不变。

## Verification

- `pack` / `unpack` 往返保持 payload 语义；schema 2/3 与损坏 zstd 帧拒绝。
- 可压缩快照的 `payload_len` 小于未压缩 inner。
- 现有 native-executable 集成测试在压缩后仍能运行。

## References

- ADR 0016 — 同宿主 native executable 为 stub + overlay
- ADR 0017 — 制品内源码快照
