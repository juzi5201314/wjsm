# `version`

打印版本信息：

```bash
wjsm version
wjsm version --extended
```

当前扩展输出包含版本号和 Rust edition。可用命令、平台 capability 与 Cranelift 版本分别以 `wjsm --help`、native compiler 初始化结果和构建依赖为准；`version` 不伪造 backend selector 或跨平台支持声明。
