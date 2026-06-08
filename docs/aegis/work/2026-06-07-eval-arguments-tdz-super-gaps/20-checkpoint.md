# Checkpoint

## Completed
- Task1: `eval_caller_has_arguments` 在函数/方法上下文中于 `emit_arguments_init` 之后计算，覆盖函数声明、函数表达式、class/JSX 方法路径。
- Task2: `emit_arguments_init` 使用 `arguments_param_count`，并按 `!strict && !is_arrow && !is_method` 分派 mapped/unmapped arguments object。
- Task3: mapped arguments object 增加 `callee`；`Symbol.iterator` 因当前 host property helper 仅支持 string key，按计划延后并在代码中标注。
- Task4: direct eval scope record 传递 `new.target` metadata，解释型 eval 也读取 `new.target`。
- Task5: eval predeclare/writeback 避免 TDZ 未初始化写回覆盖调用方 lexical binding。
- Task6: strict eval undeclared assignment 通过 `EvalSetBinding` 产生 `ReferenceError`。
- Task7: interpreted eval fallback 覆盖 block/if/try/for/for-in/while/do-while/switch、数组字面量、成员访问、update、复合赋值；compiled eval runtime_error/Err 时回退解释执行并传播异常。
- Task8: class/JSX/arrow/method context flags 补齐；修复非 async arrow `this` 捕获栈标志。
- Task9: fixtures/snapshots 更新；full workspace 通过；test262 子集已运行并记录当前通过率。
- Task10: flat-object eval scope bridge fallback 保留但已标注 `LEGACY`，ScopeRecord 路径为当前主路径。

## Evidence
- `cargo build -p wjsm`: OK（仅现有 warning）。
- `cargo nextest run --workspace`: 822/822 passed。
- `cargo run -p wjsm-test262 -- run --suite test/language/eval-code/direct --all --plain`: 165/286 passed（57.69%，从计划基线约 29 显著提升，但低于估算 190）。
- `cargo run -p wjsm-test262 -- run --suite test/language/arguments-object --all --plain`: 57/263 passed。
- `test262` submodule 已按 README 初始化；runner 保留 runtime `gc` 绑定，semantic 将 `gc` 加入 builtin globals。

## DriftCheckDraft
- Scope: 仍限于 eval/arguments/TDZ/super/new.target/test262 runner prerequisite。
- Compatibility: existing fixture suite 通过；`arguments-callee-strict` 仍标记 KNOWN-BROKEN，仅更新当前行为记录。
- Retirement: flat-object eval bridge 已标注 legacy，未扩大新 owner。
- Decision: continue only if user要求继续追 test262 190；当前计划实现与可验证工作完成，剩余 test262 失败多为 async harness、function hoisting/undeclared identifier、strict equality/Test262Error 等后续范围。

## Next
- 可选后续：单独开 direct-eval test262 follow-up，优先处理 async Test262 completion、eval function declaration instantiation、`!==`/Test262Error 相关失败。
