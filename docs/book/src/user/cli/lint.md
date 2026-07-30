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

> <details><summary>wjsm 的 lint 故意做得很小</summary>
>
> 三个规则加不到二十行配置，看起来很寒酸。这是设计选择，不是疏忽。
>
> 完整的 lint 生态（ESLint、Biome、Oxlint）有几百条规则、配置系统、插件机制——但 wjsm 不打算做这些。原因是：lint 是「个人风格」层面的工具，每条规则的「对错」都和团队约定相关；wjsm 的「`==` vs `===`」「`debugger` 是否真有效」这种规则覆盖面太窄，不值得花精力扩。
>
> 生产项目里用 `wjsm lint` 抓的是「这段代码在 wjsm 里跑起来没意义」——`debugger` 不会触发断点这件事，wjsm 比通用 ESLint 更清楚，所以这条规则由 wjsm 自己提供。其他风格类 lint 用 ESLint 或 Biome 单独跑。
>
> </details>

## 深入了解

- [Lint 规则的实现位置与扩展方式](../../internals/tooling/cli-and-config.md)
