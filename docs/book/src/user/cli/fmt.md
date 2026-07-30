# `fmt`

用 SWC 的代码生成器重新输出源码，达到格式化效果。

```bash
wjsm fmt app.ts          # 打印到标准输出
wjsm fmt app.ts -w       # 写回文件
```

## 行为

格式化过程是「解析成 AST，再由 SWC codegen 重新打印」。这决定了它的几个特点：

- 输出风格由 SWC codegen 决定，没有可配置项（没有缩进宽度、引号风格、行宽等开关）。
- 注释与源码里不影响 AST 的细节可能不被保留。
- 源码必须能成功解析，否则报解析错误且不产生输出。

示例：

```bash
wjsm fmt -e 'const x=1;;function f(){return x}'
```

实际执行需要文件参数，`fmt` 不支持 `-e`。用文件时的效果：

```text
const x = 1;
;
function f() {
    return x;
}
```

`;` 空语句被保留，因为它在 AST 中是一个真实节点。

> <details><summary>`fmt` 不是 Prettier 的替代品</summary>
>
> 看到「格式化」就期待 Prettier 那种「所有团队格式统一」的工具，但 wjsm 的 `fmt` 不是那个。它的定位是「把一个能解析的源码用 SWC 的 codegen 重打一遍」——目的是让源码经过 wjsm 解析器后能跑起来，附带得到一个稳定的输出。
>
> 真正的格式化工作交给 Prettier 或 Biome。wjsm `fmt` 的典型用途是：
>
> 1. 在生成代码的工具链里，确保产出的源码至少形式上一致；
> 2. 在 CI 里快速检查「这段代码能不能被 wjsm 解析」（不写文件直接 fmt 看会不会报错）。
>
> </details>

## 与其他命令的区别

`fmt` 不做语义检查，只要能解析就能格式化。要检查未声明标识符这类语义错误，用 [`check`](check.md)。

## 深入了解

- [SWC 解析边界与 codegen 的使用范围](../../internals/frontend/parser.md)
