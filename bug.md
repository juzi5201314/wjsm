# Known Bugs

可复现或已确认的 **未修复** 缺陷与间歇失败。  
规范/能力缺口（class setter、computed 方法、部分 ES 提案等）见 `AGENTS.md`「Note on gaps」。

---

## O2: 同步块中的对象字面量触发 async 再入

**Severity**: P2（有 workaround）  
**Status**: OPEN

在 **同步执行路径**（pending microtask、GC 压力下宿主回调等）中走 **对象字面量** 相关路径，可能触发 **async Store 上不正确再入/挂起**，输出丢失或不稳定。与 dispatch（state≥2）**无直接关系**。

- **Workaround**：`fixtures/happy/streams_byob_gc_pending_view.js` 用 `sink += "x"` 代替 `push({})` / 字面量分配。
- **试探复现**：同上 fixture 改为循环内 `sink = { x: 1 }` 或 `push({})`。

---

## 维护

- 新 bug：复现步骤 + 验证命令。  
- 修掉后：**从本文件删除**该条（不在此保留已解决档案）。  
- Workaround 写在 fixture 注释并在此交叉引用。