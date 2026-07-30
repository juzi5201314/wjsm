# 检查、格式化与 Lint

## 提交前的三步

```bash
wjsm check src/main.ts --root .
wjsm lint src/main.ts
wjsm fmt src/main.ts
```

`check` 走到 lowering 阶段，能捕获语法错误、重复声明、TDZ 访问和其他早期错误，但不执行程序，也不做 TypeScript 类型检查。

## 批量格式化

`fmt` 一次只接受一个文件，批量处理用 shell 循环：

```bash
for f in src/*.ts; do wjsm fmt -w "$f"; done
```

不带 `-w` 时把结果打到 stdout，可用于 diff 校验：

```bash
wjsm fmt src/main.ts | diff - src/main.ts
```

## Lint 规则范围

内置三条规则：`eqeq`、`neqeq`、`debugger-noop`。发现问题时退出码为 1，可直接用于 CI 门禁。规则集不可配置，也没有插件机制。

## CI 中的用法

```bash
set -e
wjsm check src/main.ts --root .
wjsm lint src/main.ts
wjsm test tests
```

三个命令都用退出码表达结果，无需解析输出。

> <details><summary>CI 里 `set -e` 和退出码配合的最佳实践</summary>
>
> 三个命令都靠退出码表达成败，CI 流水线就只需要 `set -e` 加一条退出码判断：
>
> ```bash
> set -euo pipefail
> wjsm check src/main.ts --root .
> wjsm lint src/main.ts
> wjsm test tests
> ```
>
> 加 `pipefail` 是为了防止 `wjsm xxx | tee log.txt` 这种管道里某一阶段失败被掩盖。`set -u` 捕获 typo 出来的未定义变量。
>
> 失败时不要 `exit 1`——`set -e` 已经做这件事。手工 `exit 1` 会让错误信息不明确。
>
> 想知道「是 check 失败还是 lint 失败」？用 `set -e` + 分步检查：
>
> ```bash
> if ! wjsm check ...; then
>   echo "::error::check failed" >&2
>   exit 1
> fi
> ```
>
> GitHub Actions 会把 `::error::` 捕获到 annotation 里，在 PR diff 上直接标红。
>
> </details>

## 深入了解

- [解析阶段如何产生带位置的诊断](../../internals/frontend/diagnostics-and-spans.md)
- [Hoisting、TDZ 与早期错误的判定规则](../../internals/frontend/hoisting-tdz-and-errors.md)
