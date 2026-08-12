# Summary

[首页](README.md)

---

# 用户手册

- [用户手册](user/README.md)
  - [架构与执行模型](user/overview/architecture.md)
  - [命令行](user/cli/README.md)
    - [`run`](user/cli/run.md)
    - [`build`](user/cli/build.md)
    - [`test`](user/cli/test.md)
    - [`check`](user/cli/check.md)
    - [`lint`](user/cli/lint.md)
    - [`eval`](user/cli/eval.md)
    - [`repl`](user/cli/repl.md)
    - [`fmt`](user/cli/fmt.md)
    - [`install`](user/cli/install.md)
    - [`cache`](user/cli/cache.md)
    - [`completions`](user/cli/completions.md)
    - [`init`](user/cli/init.md)
    - [`version`](user/cli/version.md)
    - [`dump-ast`](user/cli/dump-ast.md)
    - [`dump-ir`](user/cli/dump-ir.md)
    - [`dump-clif`](user/cli/dump-clif.md)
    - [`validate`](user/cli/validate.md)
    - [`size`](user/cli/size.md)
    - [`disasm`](user/cli/disasm.md)
    - [全局选项](user/cli/global-options.md)
  - [Portable `.wjsm` 制品](user/output/portable-artifacts.md)
  - [标准输出、标准错误与退出码](user/output/process-io.md)
  - [安全与资源边界](user/output/security-and-resources.md)
  - [作为 Rust 库嵌入](user/workflows/embedding.md)
  - [语言功能矩阵](user/reference/language-matrix.md)
  - [Node.js 兼容矩阵](user/reference/node-compatibility-matrix.md)

---

# 内部手册

- [内部手册](internals/README.md)
  - [端到端架构](internals/foundations/architecture.md)
  - [Workspace crate 地图](internals/foundations/crate-map.md)
  - [Direct Cranelift 后端](internals/backend/README.md)
  - [编译与执行流水线](internals/pipeline/README.md)
  - [测试与验证](internals/testing/README.md)
  - [架构决策索引](internals/reference/adr-index.md)
