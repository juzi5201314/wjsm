# 测试与验证

测试证明 observable behavior、边界、生命周期和删除结果，不通过修改 expected、ignored test 或 fallback 隐藏问题。

## 分层证据

- semantic lowering：IR snapshots 与 early-error tests；
- artifact：canonical bytes、round-trip、limits、hash、ABI、corruption；
- native backend：CLIF、trap-free gate、relocation、W^X、unwind、image lifecycle、cache invalidation；
- runtime：fixtures/happy、errors、modules、workers、async、inspector、snapshot 与 GC；
- property/Test262：在真实 native runtime 上执行；
- performance：release `wjsm-bench` cold/warm 与三 collector `wjsm-gc-bench` JSON。

## 常用命令

```bash
cargo fmt --check
cargo check --workspace
cargo nextest run --workspace
cargo nextest run -p wjsm-semantic
cargo nextest run -p wjsm-artifact-format
cargo run -- build -e 'console.log(1)' -o /tmp/hello.wjsm
cargo run -- validate /tmp/hello.wjsm
cargo run -- run /tmp/hello.wjsm
```

## 诊断顺序

```text
dump-ast → dump-ir → dump-clif → disasm → NativeRuntime/fixture
```

时序测试使用 channel gate 或其他确定性同步；不以 wall-clock sleep 证明事件顺序。缺少真实平台 runner、AVX-512、大内存或 NUMA 能力时，报告 `needs-capability-runner`，不能当作通过。

## 完成门

交付前必须同时有：

1. changed owner 的窄测试；
2. workspace build/test 与 zero-warning 证据；
3. bounded source/manifest/dependency scan；
4. artifact/cache/image lifecycle 证据；
5. 支持平台与不支持平台的 fail-closed 证据；
6. 文档与 CLI help 与当前行为一致。
