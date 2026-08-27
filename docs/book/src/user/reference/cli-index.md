# 命令行索引

所有子命令的速查表。完整参数语法以 `wjsm --help` 和 `wjsm <command> --help` 为准。全局选项可写在子命令前或后。

## 执行

| 子命令 | 用途 |
| --- | --- |
| [`run`](../cli/run.md) | 编译并立即执行 JS/TS 入口 |
| [`eval`](../cli/eval.md) | 求值单个表达式并打印结果 |
| [`repl`](../cli/repl.md) | 进入交互式逐行求值循环 |
| [`test`](../cli/test.md) | 运行测试文件或内联测试代码 |

## 构建

| 子命令 | 用途 |
| --- | --- |
| [`build`](../cli/build.md) | 把源码构建为 portable `.wjsm` 制品 |

## 源码检查

| 子命令 | 用途 |
| --- | --- |
| [`check`](../cli/check.md) | 解析和语义检查，不执行 |
| [`lint`](../cli/lint.md) | 基于 AST 的规则检查 |
| [`fmt`](../cli/fmt.md) | 用 SWC codegen 重新格式化源码 |

## 流水线观察

| 子命令 | 用途 |
| --- | --- |
| [`dump-ast`](../cli/dump-ast.md) | 打印 SWC 解析后的 AST JSON |
| [`dump-ir`](../cli/dump-ir.md) | 打印语义 lowering 之后的 IR |
| [`dump-clif`](../cli/dump-clif.md) | 打印 Cranelift IR |

诊断顺序为 `dump-ast` → `dump-ir` → `dump-clif` → `disasm`，用于逐阶段定位问题归属。

## 制品检查

| 子命令 | 用途 |
| --- | --- |
| [`validate`](../cli/validate.md) | 验证 `.wjsm` 制品完整性，不执行 |
| [`size`](../cli/size.md) | 报告 portable 制品与 native image 体积 |
| [`disasm`](../cli/disasm.md) | 反汇编当前宿主的 native image 机器码 |

## 环境管理

| 子命令 | 用途 |
| --- | --- |
| [`init`](../cli/init.md) | 创建新项目目录 |
| [`install`](../cli/install.md) | 从 npm 下载并解压包到 `node_modules/` |
| [`cache`](../cli/cache.md) | 管理 native image cache |
| [`completions`](../cli/completions.md) | 生成 shell 补全脚本 |
| [`version`](../cli/version.md) | 打印版本信息 |

## 全局选项

以下选项可作用于任何子命令：

| 选项 | 用途 |
| --- | --- |
| `--config <PATH>` | 指定配置文件 |
| `-q/--quiet` | 抑制非必要诊断 |
| `-v/--verbose` | 增加阶段诊断，可重复 |
| `--time` | 输出 pipeline timing |
| `--stats` | 输出 IR 与 artifact 统计 |
| `--verify-ir` | codegen 前验证 IR |
| `--color <auto\|always\|never>` | 控制颜色输出 |
| `--browser` | 启用 browser 模块解析条件 |
| `--condition <NAME>` | 自定义模块解析条件 |
| `--max-heap-size <SIZE>` | 设置堆内存上限 |
| `--inspect[=HOST:PORT]` | 启动 CDP inspector |
| `--inspect-brk[=HOST:PORT]` | 启动并在入口暂停 |

完整全局选项说明见[全局选项](../cli/global-options.md)。

## 深入了解

- [命令行总览](../cli/README.md)
- [全局选项](../cli/global-options.md)
