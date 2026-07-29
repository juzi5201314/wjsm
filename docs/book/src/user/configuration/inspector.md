# Inspector 调试

wjsm 内置 Chrome DevTools Protocol（CDP）服务端。启用后可以用 Chrome DevTools 或其他 CDP 客户端连接到运行中的程序。

## 启用

```bash
wjsm --inspect run app.js
wjsm --inspect=9345 run app.js
wjsm --inspect=0.0.0.0:9229 run app.js
wjsm --inspect-brk run app.js
```

两个选项都必须用 `=` 传值。写成 `--inspect 9229` 会让 clap 把 `9229` 当作子命令名。

启动后 wjsm 在标准错误打印监听地址：

```text
Debugger listening on ws://127.0.0.1:9345/38874e84-dbf3-462b-9ff1-8e0308705cb4
```

## 地址写法

| 写法 | 解析结果 |
| --- | --- |
| 省略值 | `127.0.0.1:9229` |
| `9345` | `127.0.0.1:9345` |
| `:9345` | `127.0.0.1:9345` |
| `0` 或 `:0` | `127.0.0.1` + 系统分配的临时端口 |
| `0.0.0.0:9229` | 指定主机和端口 |

非法写法会在启动前报错：

```text
Error: invalid inspect address `notaport` (expected HOST:PORT or PORT)
```

## 两个选项的区别

`--inspect` 启动服务端后立即执行程序，不等待客户端。`--inspect-brk` 在执行入口代码前暂停，直到调试器连接并恢复；没有客户端连接时程序会一直等待。

同时传两个时 `--inspect-brk` 生效，包括它给出的地址。

## 安全边界

Inspector 端口没有认证。连接上来的客户端可以读取程序状态并控制执行。默认地址 `127.0.0.1` 只接受本机连接；改成 `0.0.0.0` 会把调试通道暴露给网络上的任何人，仅在可信网络中这样做。

启用 inspector 会强制使用 Cranelift 编译器并打开调试代码生成，`WJSM_COMPILER=winch` 在这种情况下不生效。

## 深入了解

- [Inspector 服务端、CDP 域实现与 wasmtime guest_debug 的关系](../../internals/runtime-features/inspector-and-cdp.md)
- [调试代码生成如何改变编译产物](../../internals/backend/normal-and-eval-modes.md)
