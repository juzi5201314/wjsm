# 循环依赖、缓存与求值顺序

求值顺序由 `graph.rs::topological_order` 决定，它同时负责识别循环。

## DFS 后序 = 依赖优先

`topological_order` 返回 `(Vec<ModuleId>, Vec<ModuleId>)`：第一项是执行顺序，第二项是循环参与者。

遍历规则：

1. 从入口开始 DFS，保证入口可达子图先排定。
2. 处理与入口不连通的模块（正常 build 后罕见），按路径字符串排序，保证顺序确定。
3. 后序 push，因此依赖排在依赖者之前。

`VisitState` 只有 `Visiting` / `Visited` 两态。遇到 `Visiting` 节点即回边，把该 `ModuleId` 记入 `cycles` 后返回，**不报错**。这是有意的：基础循环依赖场景允许继续执行，与 ESM 的实际语义一致。

## 只沿静态边递归

递归只走 `node.imports`。动态 `import()` 目标虽然已在 BFS 阶段被发现并进入图中，但不构成静态依赖边，不参与初始化顺序。原因是动态 import 的求值时机由运行时决定，把它算进拓扑序会得到错误的初始化次序。

## 循环下的可观察行为

循环成员按拓扑序先执行的那个，会看到后执行模块的绑定尚未初始化。用户手册的多文件构建章有可复现示例：直接读会得到 `undefined`，延迟到函数调用时读则拿到正确值。这与 V8 一致，因为 ESM 绑定是引用而非拷贝。

## 解析缓存

`ModuleResolver` 内部用 `RefCell` 持有两级缓存：

- 模块 id 缓存：同一 canonical 路径只解析一次，重复 import 复用同一 `ModuleId`。
- `package_cache`：按 canonical 包目录缓存 `PackageInfo`，避免重复读 `package.json`。

这两个缓存的生命周期是单次 bundle。编译产物级缓存是另一回事，由运行时侧的 wasm 缓存负责。

> <details><summary>为什么「循环依赖」不报错？</summary>
>
> 严格说，循环依赖是 ESM 规范明确允许的——只要有绑定就位，使用方在调用时能拿到值。V8、Node、浏览器都按规范处理，wjsm 沿用相同行为。
>
> 「报错」会破坏真实项目的兼容性。npm 生态里很多包有循环依赖（特别是互相 import 类型的包），如果 wjsm 拒绝编译，这些包就用不了。
>
> 当前选择「不报错 + 暴露 TDZ 行为」是规范一致 + 用户可控：
>
> - 合法循环（互相只在函数内引用）正常工作。
> - 不合法循环（在初始化时序里直接读未初始化绑定）会读到 0（NaN-box undefined），用户能立刻发现并改写。
>
> 不在编译期拒绝的理由是：编译期静态判定无法精确分辨「读绑定 vs 调用读绑定的函数」——后者是合法的，前者可能踩雷。
>
> </details>

## 深入了解

- [循环依赖的用户可观察行为](../../user/projects/multi-file-builds.md)
- [WASM 编译缓存的键与目录](../tooling/cache.md)
- [模块图构建与解析器](graph-and-resolution.md)
