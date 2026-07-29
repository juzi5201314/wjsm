# `lint`

对源码做基于 AST 的规则检查。规则集固定，不读取外部 lint 配置文件。

```bash
wjsm lint app.ts
wjsm lint -e 'if (a == b) {}'
```

## 规则

| 规则代码 | 触发条件 | 说明 |
| --- | --- | --- |
| `eqeq` | 使用 `==` | 建议改用 `===`，避免隐式类型转换 |
| `neqeq` | 使用 `!=` | 建议改用 `!==` |
| `debugger-noop` | 出现 `debugger` 语句 | `debugger` 在 wjsm 中是编译期空操作，不会中断执行 |

## 输出与退出码

每条命中打印一行 `warning[<规则代码>]: <说明>`：

```text
warning[eqeq]: use `===` instead of `==` to avoid implicit coercion
```

有任何命中时退出码为 `1`，没有命中则为 `0`。因此可以直接放进 CI 作为门禁。

`lint` 只做规则匹配，不做类型检查，也不判断代码能否成功编译。要确认能否编译，用 [`check`](check.md)。

## 深入了解

- [Lint 规则的实现位置与扩展方式](../../internals/tooling/cli-and-config.md)
