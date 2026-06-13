# Known Bugs

可复现或已确认的 **未修复** 缺陷与间歇失败。  
规范/能力缺口（class setter、computed 方法、部分 ES 提案等）见 `AGENTS.md`「Note on gaps」。

---

## O1: TLA / async 函数内 `while` 与 continuation 状态机

**Severity**: P1  
**Status**: OPEN

含 **`while` + 多次 `await`** 时，continuation 分段或 resume 目标可能错误，后续迭代不执行或提前结束。

- 表现与「第三次及以后 resume 没跑到」类似，但 **不是** 已修的 Switch case 发射问题（`compile_switch_case`）；需查 semantic（`resolve_pending_suspends`、`build_cfg`、`lowerer_async_eval`）与循环 + Switch 交叉编译。
- **TLA**：顶层 await 模块里 while 体内多次 suspend 时尤其可疑；无 while 的 `tla_multi_await` 通过不否定本条。
- **复现线索**：`crates/wjsm-runtime/tests/fetch_http_streaming.rs::fetch_http_reader_reads_all_chunks`（`while (true) { await reader.read() }`）间歇 stdout 为空。

```bash
cargo nextest run -p wjsm-runtime -E 'test(fetch_http_reader_reads_all_chunks)'
# 建议补 fixture：TLA + while + 多段 await
```

---

## O2: 同步块中的对象字面量触发 async 再入

**Severity**: P2（有 workaround）  
**Status**: OPEN

在 **同步执行路径**（pending microtask、GC 压力下宿主回调等）中走 **对象字面量** 相关路径，可能触发 **async Store 上不正确再入/挂起**，输出丢失或不稳定。与 dispatch（state≥2）**无直接关系**。

- **Workaround**：`fixtures/happy/streams_byob_gc_pending_view.js` 用 `sink += "x"` 代替 `push({})` / 字面量分配。
- **试探复现**：同上 fixture 改为循环内 `sink = { x: 1 }` 或 `push({})`。

---

## O3: `fetch_http_reader_reads_all_chunks` 间歇空输出

**Severity**: P2  
**Status**: OPEN

期望 stdout 含 `total 7`、`reads 2`；失败时常为 **`unexpected output: ""`**。可能与 **O1**、post-main scheduler / HTTP worker / `AsyncOpGuard`（见 `docs/async-scheduler.md`）叠加；也可能在 dispatch 已修后仍单独 flake。

```bash
for i in 1 2 3 4 5; do
  cargo nextest run -p wjsm-runtime -E 'test(fetch_http_reader_reads_all_chunks)' || true
done
```

---

## 维护

- 新 bug：复现步骤 + 验证命令。  
- 修掉后：**从本文件删除**该条（不在此保留已解决档案）。  
- Workaround 写在 fixture 注释并在此交叉引用。