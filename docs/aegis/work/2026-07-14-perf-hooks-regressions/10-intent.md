# 任务意图

- 请求结果：完整执行 `local://perf-hooks-regressions-plan.md`，修复 `perf_hooks` 回归及同一方法闭包 bug class。
- 范围：对象 method/accessor、class constructor/public/private/static function 捕获环境、native entry fixture 事件门控，以及计划列出的分层验证。
- 非目标：扩展完整 Performance API、改变 runtime mask/WASM ABI/snapshot format、增加 HTTP Rust queue、加入 polling/fallback。
- 风险提示：共享工作树已有大量同一工作流改动；semantic shared env 与 class private identity 属于跨路径核心语义。

## BaselineReadSetHint

- `local://perf-hooks-regressions-plan.md`
- `/home/soeur/project/wjsm/AGENTS.md`
- `/home/soeur/.omp/agent/AGENTS.md`
- 计划引用的 ECMAScript `ClassDefinitionEvaluation` 与 `InitializeInstanceElements` 契约

## BaselineUsageDraft

- required refs：以上四项
- acknowledged refs：三份本地权威约束均已读取；ECMAScript 契约由已批准计划明确给出
- cited refs：`local://perf-hooks-regressions-plan.md`
- missing refs：无
- decision：continue

## ImpactStatementDraft

修复 semantic function value 物化和 continuation 所有权；运行时 producer/mask 保持不变。计划要求删除被统一入口替代的 raw `FunctionRef` 分支，不保留兼容路径。
