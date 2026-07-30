# completions

把 shell 补全脚本打印到标准输出。

```bash
wjsm completions <SHELL>
```

`<SHELL>` 取 `bash`、`elvish`、`fish`、`powershell`、`zsh` 之一。

脚本只写到 stdout，安装位置由你决定：

```bash
wjsm completions bash > ~/.local/share/bash-completion/completions/wjsm
wjsm completions zsh  > ~/.zfunc/_wjsm
wjsm completions fish > ~/.config/fish/completions/wjsm.fish
```

补全内容由 Clap 参数模型生成，因此和 `--help` 始终一致：新增子命令或选项后重新生成即可。
生成脚本中的命令名取自二进制名，如果你把 `wjsm` 改名安装，需要相应调整补全文件里的函数名。

> <details><summary>补全脚本为什么用 `--help` 一样的内容？</summary>
>
> 它们的来源都是 `wjsm-cli` 里的 clap `#[derive(Clap)]` 结构。`--help` 把这个结构打印成文字，补全脚本把它打印成 shell 规则——两个生成路径都从同一份参数模型出发，因此永远不会不一致。
>
> 实际的工程意义：增加新子命令时不需要单独维护补全文件，也不需要在多个地方同步文档——重新跑一次 `wjsm completions <SHELL>` 就行。
>
> 副作用是：补全脚本里出现的「选项说明」是 clap 的 `help` 字段（短句），不是手册里的详细描述。要看完整说明还是得 `wjsm <cmd> --help` 或翻手册。
>
> </details>

## 深入了解

- [CLI 参数模型与配置合并](../../internals/tooling/cli-and-config.md)
