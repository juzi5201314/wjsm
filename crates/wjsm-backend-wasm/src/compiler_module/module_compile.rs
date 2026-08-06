use super::module_bootstrap::PrologueKind;
use super::*;

/// 判定函数是否可获得 direct_call fast 入口；返回声明形参数（排除 $env/$this）。
///
/// 条件：direct_callable（不依赖 env/this/new.target、无 eval）且函数体不引用
/// `arguments`/rest 参数（fast 入口无 args_base/args_count，无法构造 arguments 对象）。
/// 形参数超过 `MAX_FAST_PARAMS` 时回退 slow 入口。
fn fast_entry_param_count(function: &IrFunction) -> Option<u32> {
    if !function.direct_callable() {
        return None;
    }
    let n = function
        .params()
        .iter()
        .filter(|p| {
            let s = p.as_str();
            s != "$env" && s != "$this" && !s.ends_with(".$env") && !s.ends_with(".$this")
        })
        .count() as u32;
    if n > MAX_FAST_PARAMS {
        return None;
    }
    for bb in function.blocks() {
        for ins in bb.instructions() {
            match ins {
                Instruction::CollectRestArgs { .. } => return None,
                Instruction::LoadVar { name, .. }
                    if name == "arguments" || name.ends_with(".arguments") =>
                {
                    return None;
                }
                // forward_args 的 super() 依赖 args_base/args_count 参数
                // （Type 12 shadow prologue），fast entry 寄存器直传无这些参数。
                Instruction::SuperCall {
                    forward_args: true, ..
                } => return None,
                _ => {}
            }
        }
    }
    Some(n)
}

/// WJSM_DISABLE_LICM 是否生效：值非空即禁用（与 WJSM_STARTUP_SNAPSHOT 的读取
/// 风格一致：除显式 0/false/off（及空值）外均视为生效），支持 1/true/on 等
/// 常见写法。
///
/// bench 需要测"真实性能"：循环内的纯 work() 不应被提升出循环（否则 fib30
/// 会被优化成空转），bench runner 在 default 档给 wjsm 子进程设 WJSM_DISABLE_LICM=1。
fn licm_disabled_by_env() -> bool {
    !matches!(
        std::env::var("WJSM_DISABLE_LICM").as_deref(),
        Err(_) | Ok("") | Ok("0") | Ok("false") | Ok("off")
    )
}

impl Compiler {
    pub(crate) fn compile_module(&mut self, module: &IrModule) -> Result<()> {
        // LICM：克隆模块，先分析（gate）再提升循环不变量纯调用。
        //
        // WJSM_DISABLE_LICM 生效时整体跳过 LICM（不做 f64/gc 预分析、不调用
        // hoist_loop_invariant_pure_calls、不设置 hoisted_preheader_blocks）：
        // bench 需要测"真实性能"，循环内的纯 work() 调用不应被提升出循环
        // （否则 fib(30) 会被优化成空转，测不到真实开销）。
        // 克隆模块供 LICM 原地变换；编译阶段统一使用该克隆，保证提升后的
        // preheader 块（含被移出循环的调用）进入 structured/cfg 编译。
        let mut module = module.clone();
        // 空跳转块消除（CFG 清洗）：语句级 is_exception 分叉的正常路径常落到一个
        // 空 continue 跳板（无指令、无 phi、仅无条件 Jump），它对循环更新块产生
        // 非循环头后向边，needs_cfg_dispatch 会把整个函数降级为 cfg 状态机
        // （每迭代多次分派，循环性能约 2 倍损失）。在 f64/gc 分析与 LICM 之前
        // 原地消除，保证后续分析/编译基于清洗后的 CFG。
        crate::compiler_control::eliminate_empty_jump_blocks(&mut module);
        if licm_disabled_by_env() {
            // 禁用 LICM：直接以模块自身做一轮 F64+Gc 分析（与"无提升复用路径"
            // 等价，不依赖任何 LICM 预分析结果）；hoisted_preheader_blocks
            // 保持默认空（无提升块）。
            let f64_analysis = crate::analysis_f64::F64Analysis::analyze(&module);
            self.f64_analysis = Some(f64_analysis.clone());
            self.gc_analysis = Some(GcAnalysis::analyze(&module, &f64_analysis));
        } else {
            let f64_for_hoist = crate::analysis_f64::F64Analysis::analyze(&module);
            let gc_for_hoist = GcAnalysis::analyze(&module, &f64_for_hoist);
            // LICM：泛化到所有函数，返回每个函数被插入的提升 preheader 块索引
            // （供 structured 编译在循环头前提前发射其指令）。
            let hoisted = crate::compiler_licm::hoist_loop_invariant_pure_calls(
                &mut module,
                &f64_for_hoist,
                &gc_for_hoist,
            );
            self.hoisted_preheader_blocks = hoisted.into_iter().collect();
            // Pass 0: f64 值类型传播分析 + 模块级 GC 分析（Step 1 / Layer 3c）。
            // LICM 无提升时 module 未被修改，复用变换前的分析结果（省一轮
            // F64+Gc 重分析，见 issue #345）；有提升才重分析——preheader 的调用
            // 结果可能被循环内的 is_exception 消费，必须用变换后的 IR 重新分析。
            if self.hoisted_preheader_blocks.is_empty() {
                self.f64_analysis = Some(f64_for_hoist);
                self.gc_analysis = Some(gc_for_hoist);
            } else {
                let f64_analysis = crate::analysis_f64::F64Analysis::analyze(&module);
                self.f64_analysis = Some(f64_analysis.clone());
                self.gc_analysis = Some(GcAnalysis::analyze(&module, &f64_analysis));
            }
        }
        // 收集源文件路径和函数源码位置映射（供运行时错误堆栈映射）。
        self.source_file = module.source_file().map(|s| s.to_string());

        // Pass 1: Register all IR functions as WASM functions.
        let mut main_wasm_idx: Option<u32> = None;
        for (i, function) in module.functions().iter().enumerate() {
            let wasm_idx = self._next_import_func;
            self.function_name_to_wasm_idx
                .insert(function.name().to_string(), wasm_idx);

            let declared_param_count = function
                .params()
                .iter()
                .filter(|p| {
                    let s = p.as_str();
                    s != "$env" && s != "$this" && !s.ends_with(".$env") && !s.ends_with(".$this")
                })
                .count() as u32;
            self.function_param_counts.push(declared_param_count);
            self.function_names.push(function.name().to_string());
            self.function_needs_prototype
                .push(function.needs_prototype());

            if is_module_entry_ir_function(function.name()) {
                if self.mode == CompileMode::Eval {
                    // eval entry: Type 3 = (scope_env: i64) -> i64 completion value
                    self.functions.function(3);
                } else {
                    // main: Type 4 = () -> i64 (返回异常值或 undefined)
                    self.functions.function(4);
                }
                main_wasm_idx = Some(wasm_idx);
            } else {
                // JS functions: Type 12 = (i64, i64, i32, i32) -> i64 (含 env_obj)
                self.functions.function(12);
            }

            self.push_func_table(wasm_idx);
            self.function_id_to_wasm_idx.insert(i as u32, wasm_idx);
            if let Some(span) = function.source_span() {
                self.source_map_entries
                    .push((wasm_idx, span.line, span.col));
            }
            self._next_import_func += 1;

            // Step 4a：direct_callable 且不引用 arguments/rest 的函数注册 fast 入口
            // （参数寄存器直传，不入函数表——仅直接 call 引用）。模块入口（main/eval
            // entry）走 compile_function，不注册 fast 入口。
            if !is_module_entry_ir_function(function.name())
                && let Some(n) = fast_entry_param_count(function)
            {
                let fast_idx = self._next_import_func;
                self.functions.function(FAST_ENTRY_TYPE_BASE + n);
                self._next_import_func += 1;
                if let Some(span) = function.source_span() {
                    self.source_map_entries
                        .push((fast_idx, span.line, span.col));
                }
                self.function_fast_entries.insert(
                    i as u32,
                    FastEntry {
                        param_count: n,
                        wasm_idx: fast_idx,
                    },
                );
            }
        }

        // Add main export (must be known now).
        let main_idx =
            main_wasm_idx.context("backend-wasm expects lowered module entry function")?;
        if self.mode == CompileMode::Eval {
            self.exports
                .export("__eval_entry", ExportKind::Func, main_idx);
        } else {
            self.exports.export("main", ExportKind::Func, main_idx);
        }

        // Reserve indices for object helper functions (so they're known during user function compilation).
        {
            let support_import_base = host_import_specs().len() as u32;
            self.bind_v2_support_helpers(support_import_base);
        }
        let arr_proto_base = self.table_base + self.function_table.len() as u32;
        for (idx, _) in array_proto_method_specs() {
            self.push_func_table(idx as u32);
        }
        self.arr_proto_table_base = arr_proto_base;

        if self.mode == CompileMode::Normal {
            // P2.2: __wjsm_init_globals — 在 bootstrap 之前由 runtime 调用，
            // 设置所有 imported globals 的初始值（heap 布局等编译期计算值）。
            // 必须在 initialize_host_post_bootstrap 之前执行，因为 host 函数
            // 依赖 heap_ptr/obj_table_ptr 等全局的正确值。
            self.init_globals_func_idx = self._next_import_func;
            self.functions.function(4); // () -> i64
            self._next_import_func += 1;
            self.exports.export(
                "__wjsm_init_globals",
                ExportKind::Func,
                self.init_globals_func_idx,
            );

            // Startup snapshot 边界：把 primordial bootstrap 与当前模块函数属性初始化拆成可单独调用的阶段。
            self.bootstrap_func_idx = self._next_import_func;
            self.functions.function(4); // () -> i64
            self._next_import_func += 1;

            self.init_function_props_func_idx = self._next_import_func;
            self.functions.function(4); // () -> i64
            self._next_import_func += 1;
            self.exports.export(
                "__wjsm_bootstrap_once",
                ExportKind::Func,
                self.bootstrap_func_idx,
            );
            self.exports.export(
                "__wjsm_init_function_props",
                ExportKind::Func,
                self.init_function_props_func_idx,
            );
        }

        // Pre-write typeof type strings to data segment start (nul-terminated)
        // 必须在编译用户函数之前设置，否则 encode_constant 会从 offset 0 开始分配字符串，
        // 随后 typeof 字符串会覆盖用户字符串数据。
        let typeof_strings: &[(u32, &str)] = &[
            (constants::TYPEOF_UNDEFINED_OFFSET, "undefined"),
            (constants::TYPEOF_OBJECT_OFFSET, "object"),
            (constants::TYPEOF_BOOLEAN_OFFSET, "boolean"),
            (constants::TYPEOF_STRING_OFFSET, "string"),
            (constants::TYPEOF_FUNCTION_OFFSET, "function"),
            (constants::TYPEOF_NUMBER_OFFSET, "number"),
            (constants::TYPEOF_SYMBOL_OFFSET, "symbol"),
            (constants::TYPEOF_BIGINT_OFFSET, "bigint"),
        ];
        for &(offset, s) in typeof_strings {
            let end = offset as usize + s.len() + 1;
            if self.string_data.len() < end {
                self.string_data.resize(end, 0);
            }
            self.string_data[offset as usize..offset as usize + s.len()]
                .copy_from_slice(s.as_bytes());
            self.string_data[offset as usize + s.len()] = 0;
            self.string_ptr_cache
                .insert(s.to_string(), self.data_base + offset);
        }

        // Pre-write property descriptor strings after typeof strings
        // 用于 Object.getOwnPropertyDescriptor 返回的描述符对象
        let prop_desc_strings: &[(u32, &str)] = &[
            (constants::PROP_DESC_VALUE_OFFSET, "value"),
            (constants::PROP_DESC_WRITABLE_OFFSET, "writable"),
            (constants::PROP_DESC_ENUMERABLE_OFFSET, "enumerable"),
            (constants::PROP_DESC_CONFIGURABLE_OFFSET, "configurable"),
            (constants::PROP_DESC_GET_OFFSET, "get"),
            (constants::PROP_DESC_SET_OFFSET, "set"),
        ];
        for &(offset, s) in prop_desc_strings {
            let end = offset as usize + s.len() + 1;
            if self.string_data.len() < end {
                self.string_data.resize(end, 0);
            }
            self.string_data[offset as usize..offset as usize + s.len()]
                .copy_from_slice(s.as_bytes());
            self.string_data[offset as usize + s.len()] = 0;
            self.string_ptr_cache
                .insert(s.to_string(), self.data_base + offset);
        }

        let promise_strings: &[(u32, &str)] = &[
            (constants::PROMISE_STATE_PENDING_OFFSET, "pending"),
            (constants::PROMISE_STATE_FULFILLED_OFFSET, "fulfilled"),
            (constants::PROMISE_STATE_REJECTED_OFFSET, "rejected"),
            (constants::PROMISE_THEN_OFFSET, "then"),
            (constants::PROMISE_CATCH_OFFSET, "catch"),
            (constants::PROMISE_FINALLY_OFFSET, "finally"),
            (constants::PROMISE_RESOLVE_OFFSET, "resolve"),
            (constants::PROMISE_REJECT_OFFSET, "reject"),
            (constants::PROMISE_ALL_OFFSET, "all"),
            (constants::PROMISE_RACE_OFFSET, "race"),
            (constants::PROMISE_ALLSETTLED_OFFSET, "allSettled"),
            (constants::PROMISE_ANY_OFFSET, "any"),
            (constants::PROMISE_CONSTRUCTOR_OFFSET, "constructor"),
            (constants::ASYNC_ITERATOR_OFFSET, "asyncIterator"),
        ];
        for &(offset, s) in promise_strings {
            let end = offset as usize + s.len() + 1;
            if self.string_data.len() < end {
                self.string_data.resize(end, 0);
            }
            self.string_data[offset as usize..offset as usize + s.len()]
                .copy_from_slice(s.as_bytes());
            self.string_data[offset as usize + s.len()] = 0;
            self.string_ptr_cache
                .insert(s.to_string(), self.data_base + offset);
        }

        // Pre-write primordial property names used by bootstrap, function-props,
        // and host post-bootstrap (Array.prototype methods, length, name,
        // toStringTag, etc.). Fixed offsets ensure name_ids are consistent
        // across different user source compilations — required for snapshot ABI.
        for (offset, s) in constants::primordial_string_offsets() {
            let end = *offset as usize + s.len() + 1;
            if self.string_data.len() < end {
                self.string_data.resize(end, 0);
            }
            self.string_data[*offset as usize..*offset as usize + s.len()]
                .copy_from_slice(s.as_bytes());
            self.string_data[*offset as usize + s.len()] = 0;
            self.string_ptr_cache
                .insert(s.to_string(), self.data_base + *offset);
        }

        // 用户字符串区起点：primordial 固定偏移之后。
        self.data_offset = constants::USER_STRING_START;

        // ── Inline cache 区（R2）─────────────────────────────────────────────
        // IC 槽必须在编译用户函数**之前**定址：属性访问点发射的是常量槽地址。
        // 因此把 IC 区固定预留在 primordial 字符串之后、用户字符串之前——
        // 用户字符串的偏移随编译增长，无法反过来给 IC 定址。
        //
        // string_data 随后 resize 到 data_offset，把整个 IC 区填零，
        // 即 kind = IC_KIND_EMPTY，无需任何运行时初始化。
        self.assign_ic_slots(&module);
        let ic_region_bytes = self.ic_slot_count * constants::IC_SLOT_SIZE;
        self.data_offset = self.ic_base + ic_region_bytes;
        // 填充 string_data 到 data_offset，确保后续用户字符串追加到正确偏移量
        self.string_data.resize(self.data_offset as usize, 0);

        // 分配 global 索引（user module 与 support 的 global 布局对齐）。
        self.func_props_global_idx = 0;
        self.heap_ptr_global_idx = 1;
        self.obj_table_global_idx = 2;
        self.obj_table_count_global_idx = 3;
        self.num_ir_functions = module.functions().len() as u32;
        self.shadow_sp_global_idx = 4;
        self.object_heap_start_global_idx = 5;
        self.num_ir_functions_global_idx = 6;
        self.shadow_stack_end_global_idx = 7;
        self.array_proto_handle_global_idx = 8;
        self.object_proto_handle_global_idx = 9;
        self.eval_var_map_ptr_global_idx = 10;
        self.eval_var_map_count_global_idx = 11;
        self.bootstrap_done_global_idx = 12;
        self.function_props_done_global_idx = 13;
        self.function_props_base_global_idx = 14;
        self.arr_proto_table_base_global_idx = 15;
        self.arr_proto_table_len_global_idx = 16;
        self.arr_proto_table_hash_global_idx = 17;
        self.heap_limit_global_idx = 18;
        self.alloc_ptr_global_idx = 19;
        self.alloc_end_global_idx = 20;
        self.gc_alloc_bytes_global_idx = 21;
        self.gc_trigger_bytes_global_idx = 22;
        self.gc_phase_global_idx = 23;
        self.good_color_global_idx = 24;
        self.barrier_buf_ptr_global_idx = 25;
        self.barrier_buf_end_global_idx = 26;

        // Record user function base index (after all imports + helpers)
        self.user_func_base_idx = self._next_import_func;
        for (function_id, function) in module.functions().iter().enumerate() {
            if is_module_entry_ir_function(function.name()) {
                self.compile_function(&module, function, wjsm_ir::FunctionId(function_id as u32))?;
            } else {
                self.compile_js_function(
                    &module,
                    function,
                    wjsm_ir::FunctionId(function_id as u32),
                    PrologueKind::Shadow,
                )?;
                // Step 4a：fast 入口体紧随 slow 入口体发射，
                // 与 Pass 1 的函数段注册顺序（slow → fast）保持一致。
                if let Some(&FastEntry { param_count, .. }) =
                    self.function_fast_entries.get(&(function_id as u32))
                {
                    self.compile_js_function(
                        &module,
                        function,
                        wjsm_ir::FunctionId(function_id as u32),
                        PrologueKind::Direct(param_count),
                    )?;
                }
            }
        }

        self.compile_number_proto_wrappers();

        // P2.2 后 heap 布局由 imported globals 显式初始化。计算 heap_start 之前
        // 必须先固化全部 data segment；否则后续追加的函数名字符串或 eval metadata
        // 会落进 object heap，被分配/GC 覆盖。
        self.finalize_eval_var_map_data();
        self.intern_data_string("length");
        self.intern_data_string("name");
        for function_name in self.function_names.clone() {
            self.intern_data_string(&function_name);
        }

        // P2.2: 提前计算 heap 布局，供 bootstrap 函数中的 emit_globals_init 使用。
        // 这些值原本在 globals 定义段中计算，现在 globals 是 import 的，
        // 需要在编译 bootstrap 之前确定初始值。
        let heap_start = (self.data_offset + (constants::HEAP_ALLOCATION_ALIGNMENT - 1))
            & !(constants::HEAP_ALLOCATION_ALIGNMENT - 1);
        let num_functions = self.num_ir_functions;
        if self.mode == CompileMode::Normal {
            {
                // 对象在 memory64 `__heap_memory`；主 memory 只承载字符串。
                // 仍在主内存地址空间预留 handle table / barrier 洞，避免任何残留
                // V1 obj_table 写入踩到字符串；但 **不要** 把 string_data 填到
                // object_heap_start——否则 data_len 虚高，动态模块 reserve 会用大段
                // 零覆盖已 intern 的字符串。
                let handle_table_entries = std::cmp::max(
                    constants::HANDLE_TABLE_MIN_ENTRIES,
                    num_functions * constants::HANDLE_TABLE_FUNCTION_ENTRY_FACTOR,
                );
                let handle_table_size = handle_table_entries * constants::HANDLE_TABLE_ENTRY_SIZE;
                let barrier_event_buf_base = heap_start + handle_table_size;
                let barrier_event_buf_end =
                    barrier_event_buf_base + constants::GC_BARRIER_EVENT_BUFFER_SIZE;
                let object_heap_start = (barrier_event_buf_end + (constants::GC_REGION_SIZE - 1))
                    & !(constants::GC_REGION_SIZE - 1);
                // 仅对齐字符串区本身；data segment 保持紧凑。
                if self.string_data.len() < heap_start as usize {
                    self.string_data.resize(heap_start as usize, 0);
                }
                self.data_offset = self.data_offset.max(heap_start);
                self.normal_init_values = Some(NormalGlobalsInit {
                    // heap_ptr 仍推进到预留区之后，供 eval/runtime 模块字符串追加。
                    heap_ptr: object_heap_start as i32,
                    obj_table_ptr: heap_start as i32,
                    shadow_sp: 0,
                    object_heap_start: object_heap_start as i32,
                    num_ir_functions: num_functions as i32,
                    shadow_stack_end: SHADOW_STACK_INITIAL_SIZE as i32,
                    eval_var_map_ptr: self.eval_var_map_ptr as i32,
                    eval_var_map_count: self.eval_var_map_count as i32,
                    arr_proto_table_base: self.arr_proto_table_base as i32,
                    arr_proto_table_len: array_proto_table_len() as i32,
                    arr_proto_table_hash: array_proto_table_hash() as i64,
                    alloc_ptr: object_heap_start as i32,
                    alloc_end: object_heap_start as i32,
                    gc_alloc_bytes: 0,
                    gc_trigger_bytes: constants::GC_INITIAL_TRIGGER_BYTES as i32,
                    gc_phase: 0,
                    good_color: 0,
                    barrier_buf_ptr: barrier_event_buf_base as i32,
                    barrier_buf_end: barrier_event_buf_end as i32,
                });
            }
        }

        // Pass 3: Compile helper functions.
        if self.mode == CompileMode::Normal {
            self.compile_init_globals_function();
            self.compile_bootstrap_once_function();
            self.compile_init_function_props_function();
        }
        // Eval / Normal 均把函数填入父模块 __table（eval 现在 import 同一张表）。
        // 私有 table + element 会在临时 Instance 销毁后让 FunctionRef 失效。
        self.elements.active(
            Some(0),
            &ConstExpr::i32_const(self.table_base as i32),
            Elements::Functions(std::borrow::Cow::Borrowed(&self.function_table)),
        );

        if self.mode == CompileMode::Eval {
            let globals = [
                ("__func_props", self.func_props_global_idx),
                ("__heap_ptr", self.heap_ptr_global_idx),
                ("__obj_table_ptr", self.obj_table_global_idx),
                ("__obj_table_count", self.obj_table_count_global_idx),
                ("__shadow_sp", self.shadow_sp_global_idx),
                ("__object_heap_start", self.object_heap_start_global_idx),
                ("__num_ir_functions", self.num_ir_functions_global_idx),
                ("__shadow_stack_end", self.shadow_stack_end_global_idx),
                ("__array_proto_handle", self.array_proto_handle_global_idx),
                ("__object_proto_handle", self.object_proto_handle_global_idx),
                ("__eval_var_map_ptr", self.eval_var_map_ptr_global_idx),
                ("__eval_var_map_count", self.eval_var_map_count_global_idx),
                ("__bootstrap_done", self.bootstrap_done_global_idx),
                ("__function_props_done", self.function_props_done_global_idx),
                ("__function_props_base", self.function_props_base_global_idx),
                (
                    "__arr_proto_table_base",
                    self.arr_proto_table_base_global_idx,
                ),
                ("__arr_proto_table_len", self.arr_proto_table_len_global_idx),
                (
                    "__arr_proto_table_hash",
                    self.arr_proto_table_hash_global_idx,
                ),
                ("__heap_limit", self.heap_limit_global_idx),
                ("__alloc_ptr", self.alloc_ptr_global_idx),
                ("__alloc_end", self.alloc_end_global_idx),
                ("__gc_alloc_bytes", self.gc_alloc_bytes_global_idx),
                ("__gc_trigger_bytes", self.gc_trigger_bytes_global_idx),
                ("__gc_phase", self.gc_phase_global_idx),
                ("__good_color", self.good_color_global_idx),
                ("__barrier_buf_ptr", self.barrier_buf_ptr_global_idx),
                ("__barrier_buf_end", self.barrier_buf_end_global_idx),
            ];
            for (name, index) in globals {
                self.exports.export(name, ExportKind::Global, index);
            }
        }
        // 数据段跳过 IC 区：wasm 线性内存按规范零初始化，而 IC 槽的初值
        // kind = IC_KIND_EMPTY 恰好是 0，因此那段字节无需进入产物。
        //
        // 这不是可选优化。IC 区按属性访问点数预留，且位于 primordial 字符串与
        // 用户字符串**之间**，照原样发射会给每个产物凭空塞进数百 KB 全零段数据
        // （实测 +268 KB），拖慢编译与实例化，足以把接近门禁的测试推过 3s 上限。
        //
        // 故分两段发射：`[0, ic_base)` 与 `[ic_end, len)`，各自再裁掉尾部零。
        let ic_end = (self.ic_base + self.ic_slot_count * constants::IC_SLOT_SIZE) as usize;
        let ic_base = self.ic_base as usize;
        let emit_segment = |start: usize, end: usize, data: &[u8], out: &mut DataSection| {
            let end = end.min(data.len());
            if start >= end {
                return;
            }
            let slice = &data[start..end];
            let Some(last) = slice.iter().rposition(|byte| *byte != 0) else {
                return;
            };
            out.active(
                0,
                &ConstExpr::i32_const((self.data_base as usize + start) as i32),
                slice[..=last].to_vec(),
            );
        };
        let string_data = std::mem::take(&mut self.string_data);
        let mut data = std::mem::replace(&mut self.data, DataSection::new());
        emit_segment(0, ic_base, &string_data, &mut data);
        emit_segment(ic_end, string_data.len(), &string_data, &mut data);
        self.data = data;
        self.string_data = string_data;
        // 编译结束，清理 LICM preheader 记录（块索引仅对本次编译有效）。
        self.hoisted_preheader_blocks.clear();
        Ok(())
    }

    /// 为**循环体内**「常量字符串键的属性读」分配 IC 槽。
    ///
    /// 编号必须与发射端的判定条件**逐字一致**（常量字符串键 + 站点在表内），
    /// 否则槽地址整体错位，快链会读到别的站点的缓存。
    ///
    /// # 为何只在循环体内发射
    ///
    /// IC 快链内联约 46 条指令。wjsm 是 AOT 编译器——产物体积是要交付的成本，
    /// 不像 JIT 那样只存在于内存中。给全部常量键站点无条件发射，实测让
    /// `node_builtin_perf_hooks` 这类大模块的 code 段涨 240 KB（+6%），
    /// 编译与实例化随之变慢，足以把接近 3s 门禁的测试推过上限。
    ///
    /// 而收益完全集中在重复执行的站点上：只跑一次的 bootstrap 属性读，
    /// 缓存永远停在「首次 miss 回填」，白付体积。循环体是「重复执行」最可靠的
    /// 静态近似，故只在其中发射，把体积花在真正摊薄得开的地方。
    fn assign_ic_slots(&mut self, module: &IrModule) {
        self.ic_sites.clear();
        self.ic_base = (self.data_offset + constants::IC_SLOT_SIZE - 1)
            & !(constants::IC_SLOT_SIZE - 1);
        // Eval 模式共享父模块的 data 段，不能自行划 IC 区。
        if self.mode != CompileMode::Normal {
            self.ic_slot_count = 0;
            return;
        }
        let mut slot = 0_u32;
        for (function_id, function) in module.functions().iter().enumerate() {
            let blocks = function.blocks();
            // 循环体块集合：用回边区间 `[header, latch]` 近似，而非精确自然循环体。
            //
            // 这里刻意不调 compute_loop_body：它按前驱表做双向可达，单个回边就要
            // O(V+E)，全函数 O(edges × (V+E))。而本判定只决定「是否值得为该站点
            // 花代码体积」，区间近似是保守的**超集**（块序号在 wjsm 里与 CFG 顺序
            // 一致，循环体必然落在回边两端之间），多标几个块顶多多发几条快链，
            // 不影响正确性——快链本身对任何输入都退回宿主完整语义。
            let mut in_loop = vec![false; blocks.len()];
            let mut any_loop = false;
            for (latch, header) in crate::compiler_licm::find_back_edges(blocks) {
                any_loop = true;
                for flag in &mut in_loop[header..=latch.min(blocks.len() - 1)] {
                    *flag = true;
                }
            }
            if !any_loop {
                continue;
            }
            // 常量字符串键的 ValueId 集合：与发射端 const_string_ptrs 的填充条件一致。
            let mut const_string_keys: std::collections::HashSet<u32> =
                std::collections::HashSet::new();
            for block in blocks {
                for instruction in block.instructions() {
                    if let Instruction::Const { dest, constant } = instruction
                        && matches!(
                            module.constants().get(constant.0 as usize),
                            Some(Constant::String(_))
                        )
                    {
                        const_string_keys.insert(dest.0);
                    }
                }
            }
            for (block_idx, block) in blocks.iter().enumerate() {
                if !in_loop[block_idx] {
                    continue;
                }
                for (instr_idx, instruction) in block.instructions().iter().enumerate() {
                    let Instruction::GetProp { key, .. } = instruction else {
                        continue;
                    };
                    if !const_string_keys.contains(&key.0) {
                        continue;
                    }
                    // IC 槽地址发射为**绝对**内存地址（`i32.const` + `i32.load`），
                    // 而 `ic_base` 源自 `data_offset`——那是相对 `data_base` 的偏移。
                    // 主模块 data_base 为 0 时两者恰好相等，但动态加载的模块
                    // data_base 非 0，漏加会让 IC 写进主模块的字符串区并损坏它。
                    self.ic_sites.insert(
                        (function_id as u32, block_idx, instr_idx),
                        self.data_base + self.ic_base + slot * constants::IC_SLOT_SIZE,
                    );
                    slot += 1;
                }
            }
        }
        self.ic_slot_count = slot;
    }

    /// P2.2: 在 main prologue 中初始化所有 imported globals。
    /// 这些值原本通过 ConstExpr 在 global 定义时设置，改为 import 后必须显式 global.set。
    /// 只在 main 函数开始时调用一次，在任何 helper 调用之前。
    fn emit_globals_init(&mut self) {
        let init = match &self.normal_init_values {
            Some(v) => *v,
            None => return,
        };
        // global 0: __func_props = 0 (deprecated)
        self.emit(WasmInstruction::I32Const(0));
        self.emit(WasmInstruction::GlobalSet(0));
        // global 1: __heap_ptr
        self.emit(WasmInstruction::I32Const(init.heap_ptr));
        self.emit(WasmInstruction::GlobalSet(1));
        // global 2: __obj_table_ptr
        self.emit(WasmInstruction::I32Const(init.obj_table_ptr));
        self.emit(WasmInstruction::GlobalSet(2));
        // global 3: __obj_table_count = 0
        self.emit(WasmInstruction::I32Const(0));
        self.emit(WasmInstruction::GlobalSet(3));
        // global 4: __shadow_sp
        self.emit(WasmInstruction::I32Const(init.shadow_sp));
        self.emit(WasmInstruction::GlobalSet(4));
        // global 5: __object_heap_start
        self.emit(WasmInstruction::I32Const(init.object_heap_start));
        self.emit(WasmInstruction::GlobalSet(5));
        // global 6: __num_ir_functions
        self.emit(WasmInstruction::I32Const(init.num_ir_functions));
        self.emit(WasmInstruction::GlobalSet(6));
        // global 7: __shadow_stack_end
        self.emit(WasmInstruction::I32Const(init.shadow_stack_end));
        self.emit(WasmInstruction::GlobalSet(7));
        // global 8: __array_proto_handle = -1 (uninitialized)
        self.emit(WasmInstruction::I32Const(-1));
        self.emit(WasmInstruction::GlobalSet(8));
        // global 9: __object_proto_handle = -1 (uninitialized)
        self.emit(WasmInstruction::I32Const(-1));
        self.emit(WasmInstruction::GlobalSet(9));
        // global 10: __eval_var_map_ptr
        self.emit(WasmInstruction::I32Const(init.eval_var_map_ptr));
        self.emit(WasmInstruction::GlobalSet(10));
        // global 11: __eval_var_map_count
        self.emit(WasmInstruction::I32Const(init.eval_var_map_count));
        self.emit(WasmInstruction::GlobalSet(11));
        // global 12: __bootstrap_done = 0
        self.emit(WasmInstruction::I32Const(0));
        self.emit(WasmInstruction::GlobalSet(12));
        // global 13: __function_props_done = 0
        self.emit(WasmInstruction::I32Const(0));
        self.emit(WasmInstruction::GlobalSet(13));
        // global 14: __function_props_base = 0
        self.emit(WasmInstruction::I32Const(0));
        self.emit(WasmInstruction::GlobalSet(14));
        // global 15: __arr_proto_table_base
        self.emit(WasmInstruction::I32Const(init.arr_proto_table_base));
        self.emit(WasmInstruction::GlobalSet(15));
        // global 16: __arr_proto_table_len
        self.emit(WasmInstruction::I32Const(init.arr_proto_table_len));
        self.emit(WasmInstruction::GlobalSet(16));
        // global 17: __arr_proto_table_hash
        self.emit(WasmInstruction::I64Const(init.arr_proto_table_hash));
        self.emit(WasmInstruction::GlobalSet(17));
        // global 18: __heap_limit = u32::MAX (runtime overrides when max_heap_size is configured)
        self.emit(WasmInstruction::I32Const(-1));
        self.emit(WasmInstruction::GlobalSet(18));
        // global 19: __alloc_ptr
        self.emit(WasmInstruction::I32Const(init.alloc_ptr));
        self.emit(WasmInstruction::GlobalSet(19));
        // global 20: __alloc_end
        self.emit(WasmInstruction::I32Const(init.alloc_end));
        self.emit(WasmInstruction::GlobalSet(20));
        // global 21: __gc_alloc_bytes
        self.emit(WasmInstruction::I32Const(init.gc_alloc_bytes));
        self.emit(WasmInstruction::GlobalSet(21));
        // global 22: __gc_trigger_bytes
        self.emit(WasmInstruction::I32Const(init.gc_trigger_bytes));
        self.emit(WasmInstruction::GlobalSet(22));
        // global 23: __gc_phase
        self.emit(WasmInstruction::I32Const(init.gc_phase));
        self.emit(WasmInstruction::GlobalSet(23));
        // global 24: __good_color
        self.emit(WasmInstruction::I32Const(init.good_color));
        self.emit(WasmInstruction::GlobalSet(24));
        // global 25: __barrier_buf_ptr
        self.emit(WasmInstruction::I32Const(init.barrier_buf_ptr));
        self.emit(WasmInstruction::GlobalSet(25));
        // global 26: __barrier_buf_end
        self.emit(WasmInstruction::I32Const(init.barrier_buf_end));
        self.emit(WasmInstruction::GlobalSet(26));
    }

    fn compile_init_globals_function(&mut self) {
        let previous_shadow_sp_scratch_idx = self.shadow_sp_scratch_idx;
        self.shadow_sp_scratch_idx = 0;
        self.current_func = Some(Function::new(vec![(1, ValType::I32)]));

        // 设置所有 imported globals 的初始值
        self.emit_globals_init();

        self.emit(WasmInstruction::I64Const(value::encode_undefined()));
        self.emit(WasmInstruction::End);

        self.codes.function(
            self.current_func
                .as_ref()
                .expect("init_globals function should be initialized"),
        );
        self.current_func = None;
        self.shadow_sp_scratch_idx = previous_shadow_sp_scratch_idx;
    }
}
