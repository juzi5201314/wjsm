# 运行脚本与应用

## 单文件脚本

```bash
wjsm run app.ts
```

入口所在目录自动成为模块解析根。相对导入、`node_modules` 查找和 Node 内置模块都可直接使用。

## 多文件项目

项目根有 `package.json` 时，从项目根运行并显式指定解析根，保证子目录里的裸包导入解析一致：

```bash
cd myapp
wjsm run --root . src/main.ts
```

## 传参给脚本

`--` 之后的参数进入 `process.argv`，索引从 2 开始：

```bash
wjsm run app.js -- --port 8080 verbose
```

```js
console.log(process.argv[2], process.argv[3], process.argv[4]);
```

## 修改后自动重跑

```bash
wjsm run --watch app.js
```

先执行一次，然后监听文件变化。`--root` 存在时递归监听整个目录，否则只监听入口文件本身。改动合并窗口是 200 毫秒。

## package.json 脚本

入口名不是已存在的文件，但匹配 `package.json` 的 `scripts` 键时，`wjsm run <name>` 转为执行该脚本，并按 `pre<name>` → `<name>` → `post<name>` 顺序运行。`node_modules/.bin` 与 wjsm 自身所在目录会加入脚本的 `PATH`。

## 组合运行时选项

全局选项可与 `run` 自由组合：

```bash
wjsm --gc g1 --max-heap-size 512M run app.ts
wjsm --browser --condition development run app.ts
```

## 深入了解

- [编译产物如何交给宿主实例化并执行](../../internals/host-runtime/instantiation-and-lifecycle.md)
- [运行时如何按需加载模块与执行上下文](../../internals/runtime-features/module-loading.md)
