# 证据记录

## 计划第 1 步：既有 RED

命令：

```bash
cargo nextest run -E 'test(happy__node_builtin_perf_hooks_api_semantics) | test(happy__node_builtin_perf_hooks_native_gc) | test(modules__node_builtin_perf_hooks_native_entries_main)'
```

结果：exit 100。

- `happy__node_builtin_perf_hooks_api_semantics`：期望 11 行 `true`；实际第 4、7、8、9、11 项为 `false`。
- `happy__node_builtin_perf_hooks_native_gc`：期望单行 `true`；实际输出 `0 undefined 0 0 0 4 4 0` 与 `false`。
- `modules__node_builtin_perf_hooks_native_entries_main`：3.012 秒硬超时，无测试输出。
- 完整输出：`artifact://3`。

## 计划第 2 步：新增边界 RED

命令：

```bash
cargo nextest run -E 'test(happy__method_closure_live_bindings) | test(happy__class_private_closure_identity)'
```

结果：exit 100，完整输出 `artifact://6`。

- object fixture：前三项为 `true`；`super`、static/public/accessor/generator capture 五项为 `false`。
- private fixture：两个 identity 断言为 `true`；共享 binding、取出调用、static private 观察三项为 `false`。

## 计划第 3 步：class function 物化入口

命令：`cargo check -p wjsm-semantic`

结果：exit 0，1 个 crate 编译，0 warnings。

- 新 canonical owner：`lowerer_classes_ts/function_values.rs`。
- constructor caller 已接收 `ensure_shared_env` continuation。
- constructor 专用旧 helper 已删除。

