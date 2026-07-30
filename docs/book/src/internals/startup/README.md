# 启动流程与 embedded artifact

由 CLI、构建脚本、宿主三方协作完成。包含 support module 预编译、GC flavor 装载、engine 配置、主模块和副本初始化、artifact 嵌入与路径管理。

## 构建期 artifact 嵌入

`build.rs` 针对三种 GC flavor 各自产出 support cwasm，与 user wasm 共用 linker。嵌入路径通过 `include_bytes!` 注入静态量，CLI 从 `runtime_support::EMBEDDED_*_SUPPORT_CWASM` 读取，装载与 user wasm 保证 linker 对齐。

## CLI 启动与配置

CLI `wjsm run/app/build` 解析 engine 配置（memory、gc、instance、inject），决定实例化参数与 GC flavor。每次启动按受支持的 flavor 清单挑选 module，丢弃未选择的副本。

## 主模块初始化

主模块实例化后，linker 保证 type section/ABI/global/index 全匹配。support cwasm 与 user wasm 同步 linker，helper 与 slot、global 索引一一对应。GC flavor 与 shadow stack 路径配套，防止引用泄漏。

## 深入了解

- [support module 生成与嵌入流程](backend/support-module.md)
- [GC flavor 装载与 linker 对齐](backend/imports-exports-and-abi.md)
- [engine 配置与 CLI 参数表](host-runtime/engine-configuration.md)
