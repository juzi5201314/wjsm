# 语言与运行时

这一部分说明 wjsm 实际能跑什么：支持的语言语法、异步模型、动态代码、Node.js 兼容表面、系统能力，以及与其他运行时相比确实存在的差异。

判断某个能力是否可用时，最可靠的办法是直接跑一遍：

```bash
wjsm run -e 'console.log(typeof structuredClone)'
```

- [JavaScript 与 TypeScript 支持](javascript-and-typescript.md)
- [异步任务与 Promise](async-and-promises.md)
- [动态代码与隔离上下文](dynamic-code-and-vm.md)
- [Node.js 兼容能力](node-compatibility.md)
- [文件系统、网络与进程能力](system-capabilities.md)
- [限制与已知差异](limitations.md)
