# Portable `.wjsm` 制品与宿主要求

`.wjsm` 是 wjsm 的可分发制品格式。它携带 verified semantic IR，不携带机器码——编译到 native image 发生在运行时，绑定当前宿主。

## 制品内容

| 组成 | 说明 |
| --- | --- |
| verified semantic IR | 经过 `Program::verify()` 校验的语义 IR |
| canonical module manifest | 模块图、required builtins、导出表 |
| semantic ABI / hash | 用于跨宿主兼容性检查 |
| 可选 source map 与 source text | 供 inspector 堆栈映射和错误定位 |

## 不包含内容

- 当前宿主机器码或 executable mapping
- Cranelift object、relocation 或 raw pointer
- native cache key / path
- startup snapshot 私有地址

因此同一 `.wjsm` 可以在支持平台间携带。运行时验证 artifact 后，由当前宿主把 IR 编译为 native image。设置了 `WJSM_CACHE_DIR` 时才会按 digest、native ABI、codegen hash、target、Cranelift 版本和 settings 查找或写入磁盘缓存。

## 宿主平台要求

native compiler 的生产 capability 是 **x86_64 Linux** 与 **x86_64 Windows**。不支持的宿主在启动阶段由 capability gate 拒绝：

```text
Error: native backend capability error: unsupported host ...
```

artifact 本身是 target-independent 的——不包含机器码，所以可以在任意机器上 `build`、`validate`，但 `run` 和 `disasm` 需要受支持的目标平台。

## 制品验证

```bash
wjsm validate /tmp/app.wjsm
```

验证包括容器 magic/version、header/section 长度与哈希、section 重叠和重复、资源上限、module manifest、required builtins、cross-reference、semantic ABI 与 IR invariants。输入损坏、截断、超限或与当前 semantic ABI 不兼容时返回退出码 1。

`validate` 不生成当前宿主机器码，也不检查 native cache。执行阶段仍会为当前宿主验证或重新生成 native image。

## 制品执行

```bash
wjsm run /tmp/app.wjsm
```

`run` 接受 `.wjsm` artifact 作为输入。运行时先验证 artifact，再由当前宿主编译为 native image 并执行。设置了 `WJSM_CACHE_DIR` 时才会按 artifact digest、native ABI hash、codegen hash、target、Cranelift 版本和 settings 查找磁盘缓存。

## 同宿主可执行文件

`--format native-executable` 把预链 `wjsm-exec` stub、`.wjsm`、预编译 `NativeObject` 与制品内源码快照打成当前宿主的 ELF/PE。它不是 portable 制品，也不能把 runtime-private object 改后缀冒充。打包失败不创建或覆盖输出文件。拷走 exe 后只读快照，不依赖构建机源码树。

## 深入了解

- [Portable `.wjsm` 制品](portable-artifacts.md)
- [`validate`](../cli/validate.md)
- [`build`](../cli/build.md)
- [Direct Cranelift 后端](../../internals/backend/README.md)
