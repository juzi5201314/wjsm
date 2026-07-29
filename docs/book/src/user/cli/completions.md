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

## 深入了解

- [CLI 参数模型与配置合并](../../internals/tooling/cli-and-config.md)
