# 解析器与语义前端

这一部分讲 `wjsm-parser` 和 `wjsm-semantic`：源码如何变成作用域正确、错误已报的 IR。

前端是 ECMAScript 语义的主要落点。作用域、TDZ、hoisting、早期错误全部在这里判定，后端只负责把已经正确的 IR 翻成机器可执行形式。前端出错，后端无从补救。

- [SWC 解析边界](parser.md)
- [两阶段 Lowering](two-phase-lowering.md)
- [作用域树、绑定与名称解析](scopes-and-bindings.md)
- [Hoisting、TDZ 与早期错误](hoisting-tdz-and-errors.md)
- [表达式与语句](expressions-and-statements.md)
- [函数、闭包与类](functions-closures-and-classes.md)
- [控制流与异常](control-flow-and-exceptions.md)
- [模块语义](module-semantics.md)
- [诊断与源码位置](diagnostics-and-spans.md)
