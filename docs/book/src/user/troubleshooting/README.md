# 故障排查

按失败发生的阶段定位问题：先看错误来自哪一层，再对症处理。

- 装不上、跑不起来：[安装与启动问题](installation.md)
- 报语法错误或标识符错误：[解析、检查与 Lowering 问题](frontend.md)
- 编译到 WASM 失败或产物异常：[编译与 WASM 问题](compilation.md)
- 程序跑起来了但出错：[运行时错误](runtime.md)
- 缓存、快照相关的异常：[快照、缓存与嵌入工件问题](artifacts.md)

判断阶段最快的方法是 `wjsm build --stage`：停在 `parse` 说明是前端问题，停在 `lower` 说明是语义问题，能出 `.wasm` 说明问题在运行时。

> <details><summary>为什么「按阶段定位」这么重要？</summary>
>
> 流水线是顺序的：parse → lower → compile → execute。每一阶段都接受上一阶段的输出，产生自己的输出。问题只能出现在某一阶段，定位到阶段就定位了 80% 的可能原因。
>
> 举个具体例子：代码里有 TypeScript 类型错误，wjsm 完全不查类型，编译顺利通过；运行时 `arr.length` 报 `undefined`。这是「运行时类型不一致」问题——根因是 wjsm 不做类型检查，调用方传错了参数。如果一开始就按阶段检查（这个错肯定在执行阶段），能省下排查 lower、codegen 的时间。
>
> 手册把故障按阶段切分，正是为了让你按这个顺序去查：先确定阶段，再读对应章节。
>
> </details>
