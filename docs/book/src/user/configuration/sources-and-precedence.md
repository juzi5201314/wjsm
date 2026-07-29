# 配置来源与优先级

同一个设置可能来自命令行、配置文件和环境变量。三者的关系不是统一的一条链，而是按选项分组的：**配置文件只与命令行比较，环境变量只与命令行比较，配置文件和环境变量之间没有交集**——因为它们控制的选项集合不重叠。

## 命令行优先于配置文件

CLI 在解析参数后读取配置文件，逐项判断该选项是否**在命令行上显式出现过**。出现过则保留命令行值，否则用文件里的值填充。

```bash
# wjsm.toml 里写了 stats = true
wjsm run app.js            # stats 生效，来自文件
wjsm --stats run app.js    # stats 生效，来自命令行
```

判定依据是「是否显式出现」，不是「是否等于默认值」。显式传一个与默认值相同的参数，同样会屏蔽配置文件。

颜色是一个整组例外：`--color` 和 `--no-color` 只要有任意一个出现在命令行上，配置文件里的 `color` 和 `no-color` 两个键就一起被忽略，不会出现一半来自命令行、一半来自文件的状态。

## 命令行优先于环境变量

GC 选择是唯一一处命令行与环境变量的显式覆盖关系：

```bash
WJSM_GC=g1 wjsm --gc zgc run app.js   # 使用 zgc
```

其余环境变量没有对应的命令行选项，或者反过来（例如 `--shadow-stack-max` 与 `WJSM_SHADOW_STACK_MAX` 各自独立生效），详见[环境变量](environment-variables.md)。

## 没有交集的部分

配置文件能表达的键只有 13 个，`--gc`、`--inspect`、`--shadow-stack-max`、`--wasmtime-memory-reservation` 都不在其中。这些选项只能通过命令行或环境变量给出，写进配置文件不会报错，但也不会生效。完整键列表见 [`wjsm.toml` 与 `wjsm.json`](project-files.md)。

## 深入了解

- [CLI 参数模型与配置合并的实现 owner](../../internals/tooling/cli-and-config.md)
