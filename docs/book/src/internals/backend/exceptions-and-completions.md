# 异常与完成记录

这一章说明 `throw` 在后端如何落地，以及 `try/catch/finally` 的展开路径如何生成。

## 异常值

异常是一个 NaN-box 值，标签 `TAG_EXCEPTION = 5`。`Throw` 终止器在后端变成对 `env.throw` 的调用，返回值带 `TAG_EXCEPTION` 标签。调用方用 `is_exception` 检查返回值，如果是异常则跳转到 catch handler 或继续传播。

这条路径不依赖 WASM 异常处理提案，全部通过返回值检查实现。代价是每个可能抛出的调用点都要有检查分支。

## try / catch / finally

`try` 块按顺序生成指令。每个可能抛出的调用后插入异常检查分支：

```wat
call $maybe_throws
local.tee $result
i64.const 0x500000000  ;; TAG_EXCEPTION << 32
i64.and
br_if $catch_handler
;; 正常路径继续
```

catch handler 接收异常值（`local.get $result`），绑定到 catch 参数。finally 块在每个 `break` / `continue` / `return` 的展开路径上被内联复制——语义层的 `emit_unwind_for_abrupt` 已经算好清理序列，后端只按它给出的层次顺序发射指令。

## abrupt completion

`break` / `continue` 在后端是普通的 `br` 指令，跳转到对应的 `block` / `loop` 出口。跨 `try-finally` 的 break 需要先执行 finally 块再跳出，这个序列在语义层已经展开，后端看到的只是线性指令序列加 `br`。

`return` 是 `return` 指令。跨 finally 的 return 同样在语义层已展开为「执行 finally → return」序列。

## 未捕获异常

异常传播到模块入口 `$module_main` 仍未被 catch 时，返回值带 `TAG_EXCEPTION`。宿主侧的 `execute_with_options` 检查入口返回值，如果是异常则打印 `Uncaught exception:` 并设置退出码 2。

> <details><summary>为什么异常值用 NaN-boxing？</summary>
>
> 异常在 wjsm 里是个 NaN-box 值（`TAG_EXCEPTION` 标签），可以走和普通值一样的传递通道：
>
> - 函数返回值可以是异常值。
> - 局部变量可以存异常值。
> - 函数形参可以传异常值。
>
> 这让「异常」和「普通值」在 IR 层是同一个东西——「带 `TAG_EXCEPTION` 标签的 i64」。后端不用为异常单独维护一套类型系统。
>
> 代价：每个可能抛出的调用都要检查返回值标签。性能上不免费，但代码上统一。
>
> 替代方案是用 WASM 原生异常（exnref、try_table）——独立类型、独立控制流。优势是后端不写检查代码，劣势是绑定特定 WASM 提案、跨宿主兼容性差。
>
> </details>

## 深入了解

- [语义层如何构造异常传播路径与清理序列](../frontend/control-flow-and-exceptions.md)
- [NaN-boxed 值表示中的 TAG_EXCEPTION](value-representation.md)
- [用户侧的未捕获异常行为](../../user/output/process-io.md)
