# ADR 0023: 全语言投机 overlay、精确 deopt 与 IR 优化器

## Status

Accepted（2026-08-29）

Amends ADR 0014 中 overlay 数量/代码体积上限、以及「deopt 回循环头」的运行时特化合同。
Supersedes ADR 0022 中 Number-only 种子、循环头 landing pad、以及中段 deopt「重做当前迭代」的合同。
不改变 ADR 0014 的唯一生产编译链、portable `.wjsm` 边界，以及禁止解释器 / Wasm / `cranelift-jit` / 第二执行后端的约束。

## Context

ADR 0022 把运行时特化收成「克隆 verified `Program` → Number/Int32 值类重建 → 同一 `NativeCompiler` → overlay native」，类型 miss 则 deopt 回 generic native。实现随后把收益门限收成参数 Number、把 deopt 粒度收成循环头，并把全 agent overlay 上限写成 64 份 / 16 MiB。热路径上的 `GetProp` 仍走 generic Shape IC 或 `NativeRuntimeOp::GetProp`，纯对象循环无法触发 overlay。

峰值执行需要与 V8 TurboFan 同构的能力：稳定反馈后由同一编译器编出投机 native；shape / 元素种类 / 调用目标 / 表示 miss 时精确回到 generic **对应指令边界**；可观察语义与关闭特化完全一致。Cranelift 只做 CLIF→机器码。

## Decision

### 1. 两份机器码，一份分发 IR

分发 `Program`（verified，与 `.wjsm` digest 绑定）永不按某次 profile 改写。generic native 永远从它编出，是唯一 deopt / OSR 锚点。

每个稳定 `CompilationKey`（函数 + 内联栈哈希 + 相关反馈槽内容哈希）克隆 `Program`，只改克隆上的目标函数体（内联把 callee 抄进 caller，不压缩 `functions` 向量，generic `FunctionId` 稳定），再经同一 `NativeCompiler` 生成 overlay image。多种 profile 并存；shape / IC epoch / `proto_generation` 变化使相关 overlay 退出选择表，当前帧走 generic。正在执行的 activation pin 住旧 image。

### 2. `wjsm-optimize` 是唯一 IR→IR 优化 owner

`wjsm-optimize` 后端无关，只依赖 `wjsm-ir`。`wjsm-semantic` 只做 AST→IR 与语言去糖（`direct_call`、`tail_self_loop`、`string_fold`、`string_concat`、`array_inline`）。AOT 在 lowering 之后调用 `optimize(Sound)`；overlay 在克隆上调用 `optimize(Speculative)`。图表示继续用现有 SSA CFG，不引入 Sea of Nodes；内部别名分析在输出前压回可 `Program::verify()` 的 IR。

优化器产出 `OptimizedUnit`：可验证 `Program`、deopt 映射、OSR 映射、反馈/IC `slot_map`。槽对齐靠显式映射表，不再靠「不删指令、不压缩块」。

内联体量：callee IR 指令 ≤192；单次编译净增 ≤1024；深度 ≤4；同一 `FunctionId` 不递归内联。超限仍做对象/算术投机。`Suspend` 边界保持 boxed continuation；suspend 之间可以投机。mapped `arguments` 不做别名投机，但不因此拒绝整函数。

### 3. 反馈向量

`NativeFeedbackSlot` 是全部投机站点的唯一 profile owner（调用/构造、属性、元素、算术比较、分配、`LoadVar`/`StoreVar`）。generic `GetProp` 的 32 字节 IC 只服务命中路径；命中时另写反馈槽。多态最多 4 态，第 5 态 megamorphic，不编 overlay。

热路径只做廉价 store，不在生成码里 enqueue。owner 线程在 `CooperativePoll` / dispatcher drain 扫过阈值的槽，把 `ShapeTable` 上相关 transition 拷成纯整数 `SpeculativeFacts` 交给 worker。worker 不接触 GC / raw pointer。`WJSM_DISABLE_SPECIALIZATION=1` 关闭全部投机，generic 语义不变。任意能证明收益的站点都编 overlay，包括纯对象循环与纯调用内联。

### 4. 精确 deopt 与 OSR

每个可能错误副作用之前插入 guard。Deopt 元数据是有序内联帧栈，每帧 `(function_id, block_id, instruction_index, 有序 live boxed i64)`。宿主重建 generic 帧后从该指令边界继续。禁止把「跳到循环头重做整轮」当作通用策略。

OSR：generic 热循环头进入当前已发布 overlay 的 `osr_map` 同位块。deopt 后该 `CompilationKey` 禁止立刻再 OSR。

overlay 代码缓存：默认 `max_bytes = clamp(32MiB, 12.5% RSS, 256MiB)`，`max_count = 4096`；`WJSM_OVERLAY_MAX_BYTES` / `WJSM_OVERLAY_MAX_COUNT` 可覆盖（`0` 表示不限）。编译队列容量 256，同 key 合并，满时丢最旧 pending。LRU + pin 语义保持。

### 5. 峰值路径覆盖全语言热区

对象形状、元素种类（packed/holey × Number/Object + dictionary；Number 槽 unboxed f64）、调用/构造内联、逃逸分析与标量替换、表示选择、load/store 消除、LICM、有反馈时的循环剥离、分配折叠、字符串/模板、闭包环境。已知单态数据属性的 overlay 热路径是 shape guard + 槽访问，不是 `NativeRuntimeOp::GetProp`。Shape 真源仍是 `ShapeTable`。

Cranelift 只做 CLIF→机器码（`ObjectModule` + `CompiledImage`：W^X、unwind、strict relocation）。禁止自写指令选择器，禁止 `cranelift-jit`。

## Consequences

- `NATIVE_ABI_VERSION` 与 semantic ABI hash 随反馈槽、resume 槽、新 IR 指令一起变化。
- ADR 0022 的 Number-only 验证条款不再是 overlay 合同；Number 标量循环仍须保持正确，但不再是特化的准入条件。
- 用户文档不承诺峰值一定快过 V8。

## Verification

- 已证明单态数据属性的 overlay IR/CLIF 无 `GetProp` dispatcher。
- shape miss、原型污染、accessor、strict 写失败、`7n+1` TypeError、holey 数组、megamorphic、deopt 后结果与 `WJSM_DISABLE_SPECIALIZATION=1` 一致。
- 逃逸对象身份与内联帧重建夹具。
- 默认 `cargo nextest run --workspace` 仍为快速正确性测试。

## References

- ADR 0014 — Direct Cranelift 与 portable `.wjsm` 终态
- ADR 0022 — 投机 typed 区（部分合同由本 ADR 取代）
- `docs/backend-implementation-guide.md`
