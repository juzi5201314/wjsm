// ExecContext 方法片段：render
macro_rules! exec_ctx_render {
    () => {
    fn render_value(&mut self, val: Value) -> String {
        crate::runtime_render::render_value(self.caller, val).unwrap_or_default()
    }
    fn value_to_display_string(&mut self, val: Value) -> String {
        crate::eval_to_string(self.caller, val)
    }
    fn json_materialize(&mut self, json_value: &wjsm_host::JsonValue) -> Value {
        let Some(env) = self.env() else {
            return value::encode_undefined();
        };
        crate::runtime_json::build_wasm_value_with_env(self.caller, &env, json_value)
    }
    fn obj_proto_to_string(&mut self, receiver: Value) -> Value {
        crate::obj_proto_to_string_impl(self.caller, receiver)
    }
    fn error_proto_to_string(&mut self, this_val: Value) -> Value {
        crate::runtime_heap::error_proto_to_string_impl(self.caller, this_val)
    }
    fn regexp_create(&mut self, pattern: String, flags: String) -> Value {
        crate::runtime_regexp::regexp_create_from_parts(self.caller, pattern, flags)
    }
    fn regexp_test(&mut self, regex: Value, str_val: Value) -> Value {
        crate::runtime_regexp::regexp_test_impl(self.caller, regex, str_val)
    }
    fn regexp_exec(&mut self, regex: Value, str_val: Value) -> Value {
        crate::runtime_regexp::regexp_exec_impl(self.caller, regex, str_val)
    }
    fn regexp_prototype(&mut self) -> Value {
        let Some(env) = self.env() else {
            return value::encode_undefined();
        };
        if !value::is_object(self.caller.data().regexp_prototype) {
            crate::runtime_heap::ensure_regexp_prototype_initialized(self.caller, &env);
        }
        self.caller.data().regexp_prototype
    }
    fn regexp_pattern_flags(&mut self, val: Value) -> Option<(String, String)> {
        if !value::is_regexp(val) {
            return None;
        }
        let handle = value::decode_regexp_handle(val) as usize;
        let table = self
            .caller
            .data()
            .regex_table
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        table
            .get(handle)
            .map(|entry| (entry.pattern.clone(), entry.flags.clone()))
    }
    fn regexp_collect_matches(
        &mut self,
        regex: Value,
        subject: &str,
        global: bool,
    ) -> Vec<RegExpMatchInfo> {
        if !value::is_regexp(regex) {
            return Vec::new();
        }
        let entry = {
            let table = self
                .caller
                .data()
                .regex_table
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            match table.get(value::decode_regexp_handle(regex) as usize) {
                Some(e) => e.clone(),
                None => return Vec::new(),
            }
        };
        let map_match = |m: regress::Match| RegExpMatchInfo {
            start: m.start(),
            end: m.end(),
            captures: (0..m.captures.len() + 1).map(|i| m.group(i)).collect(),
            named: m
                .named_groups()
                .map(|(name, range)| (name.to_string(), range))
                .collect(),
        };
        if global {
            entry.compiled.find_iter(subject).map(map_match).collect()
        } else {
            entry
                .compiled
                .find(subject)
                .map(map_match)
                .into_iter()
                .collect()
        }
    }
    fn regexp_is_global(&mut self, regex: Value) -> bool {
        if !value::is_regexp(regex) {
            return false;
        }
        let handle = value::decode_regexp_handle(regex);
        let table = self
            .caller
            .data()
            .regex_table
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        table
            .get(handle as usize)
            .map(|e| e.flags.contains('g'))
            .unwrap_or(false)
    }
    fn regexp_string_match_default(&mut self, receiver: Value, regexp: Value) -> Value {
        crate::regexp_string_match_default(self.caller, receiver, regexp)
    }
    fn regexp_string_search_default(&mut self, receiver: Value, regexp: Value) -> Value {
        crate::regexp_string_search_default(self.caller, receiver, regexp)
    }
    fn regexp_string_split_default(&mut self, receiver: Value, sep: Value, limit: Value) -> Value {
        crate::regexp_string_split_default(self.caller, receiver, sep, limit)
    }
    fn regexp_match_all_default(&mut self, this_val: Value, regexp: Value) -> Value {
        crate::regexp_match_all_default(self.caller, this_val, regexp)
    }
    fn create_date_method(&mut self, kind: &str) -> Value {
        use crate::types::DateMethodKind;
        let kind = match kind {
            "get_date" => DateMethodKind::GetDate,
            "get_day" => DateMethodKind::GetDay,
            "get_full_year" => DateMethodKind::GetFullYear,
            "get_hours" => DateMethodKind::GetHours,
            "get_milliseconds" => DateMethodKind::GetMilliseconds,
            "get_minutes" => DateMethodKind::GetMinutes,
            "get_month" => DateMethodKind::GetMonth,
            "get_seconds" => DateMethodKind::GetSeconds,
            "get_time" => DateMethodKind::GetTime,
            "get_timezone_offset" => DateMethodKind::GetTimezoneOffset,
            "get_utc_date" => DateMethodKind::GetUTCDate,
            "get_utc_day" => DateMethodKind::GetUTCDay,
            "get_utc_full_year" => DateMethodKind::GetUTCFullYear,
            "get_utc_hours" => DateMethodKind::GetUTCHours,
            "get_utc_milliseconds" => DateMethodKind::GetUTCMilliseconds,
            "get_utc_minutes" => DateMethodKind::GetUTCMinutes,
            "get_utc_month" => DateMethodKind::GetUTCMonth,
            "get_utc_seconds" => DateMethodKind::GetUTCSeconds,
            "set_date" => DateMethodKind::SetDate,
            "set_full_year" => DateMethodKind::SetFullYear,
            "set_hours" => DateMethodKind::SetHours,
            "set_milliseconds" => DateMethodKind::SetMilliseconds,
            "set_minutes" => DateMethodKind::SetMinutes,
            "set_month" => DateMethodKind::SetMonth,
            "set_seconds" => DateMethodKind::SetSeconds,
            "set_time" => DateMethodKind::SetTime,
            "set_utc_date" => DateMethodKind::SetUTCDate,
            "set_utc_full_year" => DateMethodKind::SetUTCFullYear,
            "set_utc_hours" => DateMethodKind::SetUTCHours,
            "set_utc_milliseconds" => DateMethodKind::SetUTCMilliseconds,
            "set_utc_minutes" => DateMethodKind::SetUTCMinutes,
            "set_utc_month" => DateMethodKind::SetUTCMonth,
            "set_utc_seconds" => DateMethodKind::SetUTCSeconds,
            "to_string" => DateMethodKind::ToString,
            "to_date_string" => DateMethodKind::ToDateString,
            "to_time_string" => DateMethodKind::ToTimeString,
            "to_locale_string" => DateMethodKind::ToLocaleString,
            "to_locale_date_string" => DateMethodKind::ToLocaleDateString,
            "to_locale_time_string" => DateMethodKind::ToLocaleTimeString,
            "to_iso_string" => DateMethodKind::ToISOString,
            "to_utc_string" => DateMethodKind::ToUTCString,
            "to_json" => DateMethodKind::ToJSON,
            "value_of" => DateMethodKind::ValueOf,
            _ => return value::encode_undefined(),
        };
        crate::runtime_builtins::create_date_method(self.caller.data(), kind)
    }
    fn date_read_ms(&mut self, this: Value) -> f64 {
        crate::runtime_date::read_date_ms(self.caller, this)
    }
    fn date_args_to_ms(&mut self, args: &[Value], is_utc: bool) -> f64 {
        crate::runtime_date::date_args_to_ms(args, is_utc)
    }
    fn date_now_ms(&mut self) -> f64 {
        chrono::Utc::now().timestamp_millis() as f64
    }
    fn new_target(&mut self) -> Value {
        self.caller
            .data()
            .new_target
            .load(std::sync::atomic::Ordering::Relaxed)
    }
    fn set_date_prototype(&mut self, obj: Value) {
        if let Some(proto) = crate::runtime_heap::native_callable_date_prototype(
            self.caller,
            &crate::types::NativeCallable::DateConstructorGlobal,
        ) {
            self.set_object_proto(obj, proto);
        }
    }
    fn create_string_primitive_method(&mut self, method: u8) -> Value {
        crate::create_native_callable(
            self.caller.data(),
            crate::NativeCallable::StringPrimitiveMethod { method },
        )
    }
    fn create_number_primitive_method(&mut self, method: u8) -> Value {
        crate::create_native_callable(
            self.caller.data(),
            crate::types::NativeCallable::NumberPrimitiveMethod { method },
        )
    }
    fn create_bigint_primitive_method(&mut self, method: u8) -> Value {
        crate::create_native_callable(
            self.caller.data(),
            crate::types::NativeCallable::BigIntPrimitiveMethod { method },
        )
    }
    fn store_bigint(&mut self, n: num_bigint::BigInt) -> Value {
        let mut table = self
            .caller
            .data()
            .bigint_table
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let handle = table.len() as u32;
        table.push(n);
        value::encode_bigint_handle(handle)
    }
    fn read_bigint(&mut self, val: Value) -> Option<num_bigint::BigInt> {
        if !value::is_bigint(val) {
            return None;
        }
        let handle = value::decode_bigint_handle(val) as usize;
        let table = self
            .caller
            .data()
            .bigint_table
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        table.get(handle).cloned()
    }
    fn create_global_builtin(&mut self, name: &str) -> Option<Value> {
        use crate::types::{NativeCallable, TypedArrayConstructorKind};
        let callable = match name {
            "Array" => NativeCallable::ArrayConstructor,
            "Object" => NativeCallable::ObjectConstructor,
            "Function" => NativeCallable::FunctionConstructor,
            "String" => NativeCallable::StringConstructor,
            "Boolean" => NativeCallable::BooleanConstructor,
            "Number" => NativeCallable::NumberConstructor,
            "Symbol" => NativeCallable::SymbolConstructor,
            "BigInt" => NativeCallable::BigIntConstructor,
            "RegExp" => NativeCallable::RegExpConstructor,
            "Error" => NativeCallable::ErrorConstructor,
            "TypeError" => NativeCallable::TypeErrorConstructor,
            "RangeError" => NativeCallable::RangeErrorConstructor,
            "SyntaxError" => NativeCallable::SyntaxErrorConstructor,
            "ReferenceError" => NativeCallable::ReferenceErrorConstructor,
            "URIError" => NativeCallable::URIErrorConstructor,
            "EvalError" => NativeCallable::EvalErrorConstructor,
            "AggregateError" => NativeCallable::AggregateErrorConstructor,
            "Map" => NativeCallable::MapConstructor,
            "Set" => NativeCallable::SetConstructor,
            "WeakMap" => NativeCallable::WeakMapConstructor,
            "WeakSet" => NativeCallable::WeakSetConstructor,
            "WeakRef" => NativeCallable::WeakRefConstructor,
            "FinalizationRegistry" => NativeCallable::FinalizationRegistryConstructor,
            "Date" => NativeCallable::DateConstructorGlobal,
            "Promise" => NativeCallable::PromiseConstructor,
            "Headers" => NativeCallable::HeadersConstructor,
            "Request" => NativeCallable::RequestConstructor,
            "Response" => NativeCallable::ResponseConstructor,
            "ReadableStream" => NativeCallable::ReadableStreamConstructor,
            "WritableStream" => NativeCallable::WritableStreamConstructor,
            "TransformStream" => NativeCallable::TransformStreamConstructor,
            "CountQueuingStrategy" => NativeCallable::CountQueuingStrategyConstructor,
            "ByteLengthQueuingStrategy" => NativeCallable::ByteLengthQueuingStrategyConstructor,
            "AbortController" => NativeCallable::AbortControllerConstructor,
            "ArrayBuffer" => NativeCallable::ArrayBufferConstructorGlobal,
            "SharedArrayBuffer" => NativeCallable::SharedArrayBufferConstructor,
            "Atomics" => NativeCallable::AtomicsGlobal,
            "DataView" => NativeCallable::DataViewConstructorGlobal,
            "Int8Array" => NativeCallable::TypedArrayConstructor(TypedArrayConstructorKind::Int8),
            "Uint8Array" => NativeCallable::TypedArrayConstructor(TypedArrayConstructorKind::Uint8),
            "Uint8ClampedArray" => {
                NativeCallable::TypedArrayConstructor(TypedArrayConstructorKind::Uint8Clamped)
            }
            "Int16Array" => NativeCallable::TypedArrayConstructor(TypedArrayConstructorKind::Int16),
            "Uint16Array" => {
                NativeCallable::TypedArrayConstructor(TypedArrayConstructorKind::Uint16)
            }
            "Int32Array" => NativeCallable::TypedArrayConstructor(TypedArrayConstructorKind::Int32),
            "Uint32Array" => {
                NativeCallable::TypedArrayConstructor(TypedArrayConstructorKind::Uint32)
            }
            "Float32Array" => {
                NativeCallable::TypedArrayConstructor(TypedArrayConstructorKind::Float32)
            }
            "Float64Array" => {
                NativeCallable::TypedArrayConstructor(TypedArrayConstructorKind::Float64)
            }
            "BigInt64Array" => {
                NativeCallable::TypedArrayConstructor(TypedArrayConstructorKind::BigInt64)
            }
            "BigUint64Array" => {
                NativeCallable::TypedArrayConstructor(TypedArrayConstructorKind::BigUint64)
            }
            "Proxy" => NativeCallable::ProxyConstructor,
            "gc" => NativeCallable::GcCollect,
            "agent_start" => NativeCallable::AgentStart,
            "agent_broadcast" => NativeCallable::AgentBroadcast,
            "agent_receive_broadcast" => NativeCallable::AgentReceiveBroadcast,
            "agent_get_report" => NativeCallable::AgentGetReport,
            "agent_report" => NativeCallable::AgentReport,
            "agent_sleep" => NativeCallable::AgentSleep,
            "agent_monotonic_now" => NativeCallable::AgentMonotonicNow,
            _ => return None,
        };
        Some(crate::create_native_callable(self.caller.data(), callable))
    }
    fn native_eval_function_param_count(&mut self, val: Value) -> Option<usize> {
        if !value::is_native_callable(val) {
            return None;
        }
        let idx = value::decode_native_callable_idx(val) as usize;
        let table = self
            .caller
            .data()
            .native_callables
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        match table.get(idx) {
            Some(crate::types::NativeCallable::EvalFunction(func)) => Some(func.params.len()),
            _ => None,
        }
    }
    fn is_process_hrtime_callable(&mut self, val: Value) -> bool {
        if !value::is_native_callable(val) {
            return false;
        }
        let idx = value::decode_native_callable_idx(val) as usize;
        let table = self
            .caller
            .data()
            .native_callables
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        matches!(
            table.get(idx),
            Some(crate::types::NativeCallable::ProcessHrtime)
        )
    }
    fn create_process_hrtime_bigint(&mut self) -> Value {
        crate::create_native_callable(
            self.caller.data(),
            crate::types::NativeCallable::ProcessHrtimeBigint,
        )
    }
    };
}
