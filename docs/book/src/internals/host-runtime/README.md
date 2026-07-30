# HostRuntime 与 ExecContext

作为宿主接口，承载 JS 特性、内建能力与后端分离。接口按 ADR 0011-0013 设计，统一泛型上下文和单态化调用路径。

## ExecContext trait

`host-runtime/exec-context-and-builtins.md` 详细记录 trait 330+方法，无惰性分派。全部内建能力都要实现，后端能力用单态化路径统一。

benefits：
- 内建 Promise、Proxy、Streams、Fetch、JSON 均只用 trait 泛型实现。
- 运行时自身只关心类型边界、不绑定具体后端。
- CLI、test262、embedded engine 均走统一接口。

## builtins 解耦

builtins crate 只依赖 ExecContext 种类，不绑定后端。所有内建算法、promise调度、proxy策略都能被任何后端复用，未来多后端接入零代码迁移。

## engine 配置

engine 统一管理 GC flavor、shadow memory、artifact 路径，启动时一次性注入。所有 config 设计见 startup/README。

## 深入了解

- [ExecContext 与 builtins 分离设计](exec-context-and-builtins.md)
- [backend/多后端边界](../backend/multi-backend-boundary.md)
- [engine 配置、启动流程](startup/README.md)
