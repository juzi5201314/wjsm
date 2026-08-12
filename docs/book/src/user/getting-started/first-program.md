# 运行第一个程序

## 最短的运行

```bash
wjsm run -e 'console.log(1 + 2)'
```

输出 `3`。`-e` 把内联源码当作入口编译并执行，不落盘。脚本名在 `process.argv` 和错误信息中显示为 `[run-eval]`。

## 运行文件

```bash
echo "console.log('hello')" > hello.js
wjsm run hello.js
```

输出 `hello`。`.js` 按 ES 模块解析；`.ts` 走 TypeScript 语法。扩展名决定语法模式，详见 [文件、内联源码与标准输入](input-modes.md)。

## 构建制品

`build` 把源码编译为 portable `.wjsm` 制品，不在当前进程执行：

```bash
wjsm build -e 'console.log(1)' -o /tmp/one.wjsm
```

产物只保存 target-independent semantic IR 与 metadata，不含机器码。可以在任何受支持平台的宿主上运行，无需重新编译源码。

## 运行制品

```bash
wjsm run /tmp/one.wjsm
```

`run` 检测到输入是 `.wjsm` 制品时，跳过解析和 lowering，直接把 IR 编译为当前宿主的 native image 并执行。这就是「构建在前，执行在后」的实际形态：构建产物跨平台携带，执行时才绑定到具体宿主。

## 初始化项目

```bash
wjsm init myapp
cd myapp
wjsm run main.js
```

`wjsm init` 生成 `package.json` 和 `main.js` 两个文件，可以直接运行。详见 [初始化项目](project-init.md)。

## 深入了解

- [文件、内联源码与标准输入](input-modes.md)
- [`run` 命令](../cli/run.md)
- [`build` 命令](../cli/build.md)
