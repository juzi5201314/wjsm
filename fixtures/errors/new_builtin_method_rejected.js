// 未捕获的非构造器 construct 调用：顶层 TypeError 文案按源级 callsite
// 渲染（V8 CallPrinter 同型），与 Node 一致。
new Math.max();
