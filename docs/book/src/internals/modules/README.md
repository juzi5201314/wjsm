# 模块系统与 Bundler

`wjsm-module` 负责把多个源文件收敛为一个 IR `Program`。它介于语义前端和后端之间：向下调用 `wjsm-parser`/`wjsm-semantic`，向上只交出 IR，不接触任何 WASM 类型。

本部分覆盖依赖图构建、ESM 链接、CommonJS 转换、包条件解析、循环处理与最终的 Program bundling。
