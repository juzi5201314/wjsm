# Known Bugs

可复现或已确认的 **未修复** 缺陷与间歇失败。  
规范/能力缺口（class setter、computed 方法、部分 ES 提案等）见 `AGENTS.md`「Note on gaps」。

---

## O2: 同步块中的对象字面量触发 async 再入

**Severity**: P2（有 workaround）  
**Status**: FIXED

**根因**：`$obj_new` 和 `$arr_new` 中的 GC 触发逻辑（proactive GC 和 OOM GC）在同步执行路径中调用 `gc_collect`，导致活跃 WASM 局部变量中的对象指针被异步挂起和重新进入，破坏了 Store 的线性语义。

**修复**：将 `$obj_new` 和 `$arr_new` 中的 GC 触发替换为 `memory.grow`，确保内存扩展是同步操作，不会触发 async 再入。保留 `$obj_delete` 中的 GC 调用（删除操作不会持有活跃局部变量）。

**验证**：`fixtures/happy/streams_byob_gc_pending_view.js` 现在使用对象字面量分配（`sink.push({ x: i })`），测试通过。

---

## 维护

- 新 bug：复现步骤 + 验证命令。  
- 修掉后：**从本文件删除**该条（不在此保留已解决档案）。  
- Workaround 写在 fixture 注释并在此交叉引用。