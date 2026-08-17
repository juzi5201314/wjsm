# `validate`

验证 portable `.wjsm`，但不执行程序：

```bash
wjsm validate /tmp/app.wjsm
```

验证包括容器 magic/version、header/section 长度与哈希、section 重叠和重复、资源上限、module manifest、required builtins、cross-reference、semantic ABI 与 IR invariants。输入损坏、截断、超限或与当前 semantic ABI 不兼容时返回退出码 1。

`validate` 不生成当前宿主机器码，也不检查 native cache。执行阶段仍会为当前宿主验证或重新生成 native image。

## 深入了解

- [Portable `.wjsm` 制品](../output/portable-artifacts.md)
- [Artifact 校验与尺寸分析](../../internals/tooling/validation-and-size.md)
