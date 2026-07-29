# 故障排查

按失败发生的阶段定位问题：先看错误来自哪一层，再对症处理。

- 装不上、跑不起来：[安装与启动问题](installation.md)
- 报语法错误或标识符错误：[解析、检查与 Lowering 问题](frontend.md)
- 编译到 WASM 失败或产物异常：[编译与 WASM 问题](compilation.md)
- 程序跑起来了但出错：[运行时错误](runtime.md)
- 缓存、快照相关的异常：[快照、缓存与嵌入工件问题](artifacts.md)

判断阶段最快的方法是 `wjsm build --stage`：停在 `parse` 说明是前端问题，停在 `lower` 说明是语义问题，能出 `.wasm` 说明问题在运行时。
