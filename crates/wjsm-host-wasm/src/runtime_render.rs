use super::*;
use crate::runtime_string::RuntimeString;

fn read_string_bytes_with_env<C: AsContext>(ctx: &C, env: &WasmEnv, ptr: u32) -> Vec<u8> {
    let data = env.memory.data(ctx);
    let start = ptr as usize;
    if start >= data.len() {
        return Vec::new();
    }
    let end = data[start..]
        .iter()
        .position(|byte| *byte == 0)
        .map_or(data.len(), |offset| start + offset);
    data[start..end].to_vec()
}

fn read_string_with_env<C: AsContext>(ctx: &C, env: &WasmEnv, ptr: u32) -> Result<String> {
    Ok(std::str::from_utf8(&read_string_bytes_with_env(ctx, env, ptr))?.to_owned())
}

/// 与 `render_value` 相同的人类可读规则，供 drain 后 Store 上下文报告 unhandled rejection。
pub(crate) fn render_unhandled_rejection_reason_with_env<
    C: AsContextMut<Data = RuntimeState> + RuntimeStateAccess,
>(
    ctx: &mut C,
    env: &WasmEnv,
    val: i64,
) -> String {
    if value::is_exception(val) {
        let reason = exception_reason_from_state(ctx.state_mut(), val);
        return render_unhandled_rejection_reason_with_env(ctx, env, reason);
    }
    if value::is_string(val) {
        return read_runtime_string_with_env_lossy(ctx, env, val);
    }
    if value::is_object(val) || value::is_array(val) {
        if let Some(op) = resolve_handle_with_env(ctx, env, val)
            && let Some(brand_val) =
                read_object_property_by_name_with_env(ctx, env, op, "__error_brand__")
            && value::is_bool(brand_val)
            && value::decode_bool(brand_val)
        {
            let name = read_object_property_by_name_with_env(ctx, env, op, "name")
                .map(|name_val| render_unhandled_rejection_reason_with_env(ctx, env, name_val))
                .unwrap_or_default();
            let message = read_object_property_by_name_with_env(ctx, env, op, "message")
                .map(|message_val| {
                    render_unhandled_rejection_reason_with_env(ctx, env, message_val)
                })
                .unwrap_or_default();
            if message.is_empty() {
                return name;
            }
            return format!("{name}: {message}");
        }
        return "[object Object]".to_string();
    }
    if value::is_undefined(val) {
        return "undefined".to_string();
    }
    if value::is_null(val) {
        return "null".to_string();
    }
    if value::is_bool(val) {
        return if value::decode_bool(val) {
            "true".to_string()
        } else {
            "false".to_string()
        };
    }
    if value::is_f64(val) {
        let n = value::decode_f64(val);
        if n.is_infinite() {
            return if n.is_sign_positive() {
                "Infinity".to_string()
            } else {
                "-Infinity".to_string()
            };
        }
        return n.to_string();
    }
    format!("0x{:016x}", val as u64)
}

/// 值渲染 — 薄委托层（算法在 `wjsm-builtins::render`）。
pub(crate) fn render_value(caller: &mut Caller<'_, RuntimeState>, val: i64) -> Result<String> {
    let mut ctx = crate::exec_context_impl::WasmExecContext::new(caller);
    Ok(wjsm_builtins::render::render_value_impl(&mut ctx, val))
}

/// console 单值输出 — 薄委托层。
pub(crate) fn write_console_value(
    caller: &mut Caller<'_, RuntimeState>,
    val: i64,
    prefix: Option<&str>,
) {
    let mut ctx = crate::exec_context_impl::WasmExecContext::new(caller);
    wjsm_builtins::render::write_console_value_impl(&mut ctx, val, prefix);
}

/// 完整的 JSON.stringify（ES §24.5.2）— 薄委托层。
pub(crate) async fn runtime_json_stringify_full_async(
    caller: &mut Caller<'_, RuntimeState>,
    val: i64,
    replacer: i64,
    space: i64,
) -> i64 {
    let mut ctx = crate::exec_context_impl::WasmExecContext::new(caller);
    wjsm_builtins::render::json_stringify_full_impl(&mut ctx, val, replacer, space).await
}

pub(crate) fn read_string(caller: &mut Caller<'_, RuntimeState>, ptr: u32) -> Result<String> {
    let data = read_string_bytes(caller, ptr);
    Ok(std::str::from_utf8(&data)?.to_owned())
}

pub(crate) fn read_runtime_string(
    caller: &mut Caller<'_, RuntimeState>,
    val: i64,
) -> RuntimeString {
    if value::is_runtime_string_handle(val) {
        let handle = value::decode_runtime_string_handle(val) as usize;
        let strings = caller
            .data()
            .runtime_strings
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        strings.get(handle).cloned().unwrap_or_default()
    } else if value::is_string(val) {
        RuntimeString::from_utf8_lossy(&read_string_bytes(caller, value::decode_string_ptr(val)))
    } else {
        RuntimeString::empty()
    }
}

pub(crate) fn read_runtime_string_utf8_lossy(
    caller: &mut Caller<'_, RuntimeState>,
    val: i64,
) -> String {
    read_runtime_string(caller, val).to_utf8_lossy()
}

fn read_runtime_string_with_env_lossy<C: AsContextMut<Data = RuntimeState> + RuntimeStateAccess>(
    ctx: &mut C,
    env: &WasmEnv,
    val: i64,
) -> String {
    if value::is_runtime_string_handle(val) {
        let handle = value::decode_runtime_string_handle(val) as usize;
        let strings = ctx
            .state_mut()
            .runtime_strings
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        return strings
            .get(handle)
            .map(RuntimeString::to_utf8_lossy)
            .unwrap_or_default();
    }
    if value::is_string(val) {
        return read_string_with_env(ctx, env, value::decode_string_ptr(val)).unwrap_or_default();
    }
    String::new()
}

pub(crate) fn read_string_bytes_mem<C: AsContext>(ctx: &C, memory: &Memory, ptr: u32) -> Vec<u8> {
    let data = memory.data(ctx);
    let start = ptr as usize;
    if start >= data.len() {
        return Vec::new();
    }

    let end = data[start..]
        .iter()
        .position(|byte| *byte == 0)
        .map_or(data.len(), |offset| start + offset);

    data[start..end].to_vec()
}

pub(crate) fn read_string_bytes(caller: &mut Caller<'_, RuntimeState>, ptr: u32) -> Vec<u8> {
    let Some(Extern::Memory(memory)) = caller.get_export("memory") else {
        return Vec::new();
    };
    read_string_bytes_mem(caller, &memory, ptr)
}

pub(crate) fn read_value_string_utf8_lossy_bytes(
    caller: &mut Caller<'_, RuntimeState>,
    val: i64,
) -> Option<Vec<u8>> {
    value::is_string(val).then(|| read_runtime_string(caller, val).to_utf8_lossy_bytes())
}

pub(crate) fn read_value_string_bytes(
    caller: &mut Caller<'_, RuntimeState>,
    val: i64,
) -> Option<Vec<u8>> {
    read_value_string_utf8_lossy_bytes(caller, val)
}

pub(crate) fn read_i32_global_from_caller(
    caller: &mut Caller<'_, RuntimeState>,
    name: &str,
) -> Option<i32> {
    caller
        .get_export(name)
        .and_then(Extern::into_global)
        .and_then(|global| global.get(&mut *caller).i32())
}

pub(crate) fn read_u32_le(data: &[u8], offset: usize) -> Option<u32> {
    let bytes = data.get(offset..offset + 4)?;
    Some(u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
}

pub(crate) fn read_utf8_slice(data: &[u8], ptr: u32, len: u32) -> Option<String> {
    let start = ptr as usize;
    let end = start.checked_add(len as usize)?;
    let bytes = data.get(start..end)?;
    std::str::from_utf8(bytes).ok().map(ToOwned::to_owned)
}

pub(crate) fn read_eval_var_map(caller: &mut Caller<'_, RuntimeState>) -> Vec<EvalVarMapEntry> {
    const RECORD_SIZE: usize = 20;

    let ptr = read_i32_global_from_caller(caller, "__eval_var_map_ptr").unwrap_or(0);
    let count = read_i32_global_from_caller(caller, "__eval_var_map_count").unwrap_or(0);
    if ptr <= 0 || count <= 0 {
        return Vec::new();
    }

    let Some(Extern::Memory(memory)) = caller.get_export("memory") else {
        return Vec::new();
    };
    let data = memory.data(&*caller);
    let mut entries = Vec::with_capacity(count as usize);

    for index in 0..count as usize {
        let Some(record_offset) = (ptr as usize).checked_add(index * RECORD_SIZE) else {
            break;
        };
        let Some(function_ptr) = read_u32_le(data, record_offset) else {
            break;
        };
        let Some(function_len) = read_u32_le(data, record_offset + 4) else {
            break;
        };
        let Some(var_ptr) = read_u32_le(data, record_offset + 8) else {
            break;
        };
        let Some(var_len) = read_u32_le(data, record_offset + 12) else {
            break;
        };
        let Some(offset) = read_u32_le(data, record_offset + 16) else {
            break;
        };
        let Some(function_name) = read_utf8_slice(data, function_ptr, function_len) else {
            continue;
        };
        let Some(var_name) = read_utf8_slice(data, var_ptr, var_len) else {
            continue;
        };
        entries.push(EvalVarMapEntry {
            function_name,
            var_name,
            offset,
        });
    }

    entries
}

pub(crate) fn store_runtime_string<S>(caller: &Caller<'_, RuntimeState>, string: S) -> i64
where
    S: Into<RuntimeString>,
{
    let mut strings = caller
        .data()
        .runtime_strings
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let handle = strings.len() as u32;
    strings.push(string.into());
    value::encode_runtime_string_handle(handle)
}

pub(crate) fn store_runtime_string_in_state<S>(state: &RuntimeState, string: S) -> i64
where
    S: Into<RuntimeString>,
{
    let mut strings = state
        .runtime_strings
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let handle = strings.len() as u32;
    strings.push(string.into());
    value::encode_runtime_string_handle(handle)
}

// 纯数字格式化已迁至 wjsm-builtins；此处再导出保持 `runtime_render::*` 调用路径。
pub(crate) use wjsm_builtins::{
    format_number_js, format_number_to_exponential_js, format_number_to_fixed_js,
    format_number_to_precision_js,
};

pub(crate) fn format_radix(mut value: i64, radix: u32) -> String {
    if value == 0 {
        return "0".to_string();
    }
    let negative = value < 0;
    if negative {
        value = -value;
    }
    let digits = b"0123456789abcdefghijklmnopqrstuvwxyz";
    let mut result = Vec::new();
    while value > 0 {
        result.push(digits[value as usize % radix as usize]);
        value /= radix as i64;
    }
    if negative {
        result.push(b'-');
    }
    result.reverse();
    String::from_utf8(result).unwrap_or_else(|_| "0".to_string())
}

/// `Number.prototype.toString(radix)`：整数部分 + 小数部分（乘 radix 取整），最多 52 位小数。
pub(crate) fn format_f64_radix_to_string(x: f64, radix: i32) -> String {
    if x == 0.0 && !x.is_sign_negative() {
        return "0".to_string();
    }
    let radix_u = radix as u32;
    let negative = x.is_sign_negative();
    let abs_x = x.abs();
    let int_part = abs_x.trunc() as i64;
    let mut int_str = if int_part == 0 {
        "0".to_string()
    } else {
        format_radix(int_part, radix_u)
    };
    let mut frac = abs_x - abs_x.trunc();
    if frac > 0.0 {
        int_str.push('.');
        let digits = b"0123456789abcdefghijklmnopqrstuvwxyz";
        const MAX_FRAC_DIGITS: usize = 52;
        for _ in 0..MAX_FRAC_DIGITS {
            if frac == 0.0 {
                break;
            }
            frac *= radix_u as f64;
            let digit = frac.trunc() as usize;
            if digit >= radix as usize {
                break;
            }
            int_str.push(digits[digit] as char);
            frac -= digit as f64;
        }
    }
    if negative {
        format!("-{int_str}")
    } else {
        int_str
    }
}

pub(crate) fn find_memory_c_string_global(
    caller: &mut Caller<'_, RuntimeState>,
    name: &str,
) -> Option<u32> {
    let mut needle = Vec::with_capacity(name.len() + 1);
    needle.extend_from_slice(name.as_bytes());
    needle.push(0);
    let Some(Extern::Memory(memory)) = caller.get_export("memory") else {
        return None;
    };
    memory
        .data(&*caller)
        .windows(needle.len())
        .position(|window| window == needle.as_slice())
        .map(|offset| offset as u32)
}

pub(crate) fn alloc_heap_c_string_global(
    caller: &mut Caller<'_, RuntimeState>,
    name: &str,
) -> Option<u32> {
    let env = WasmEnv::from_caller(caller)?;
    let heap_ptr = env.heap_ptr.get(&mut *caller).i32().unwrap_or(0) as usize;
    let bytes = name.as_bytes();
    let end = heap_ptr.checked_add(bytes.len() + 1)?;
    let aligned_end = (end + 7) & !7;
    if !crate::runtime_heap::ensure_heap_allocation_bytes(
        caller,
        &env,
        heap_ptr,
        aligned_end.saturating_sub(heap_ptr),
    ) {
        return None;
    }
    {
        let data = env.memory.data_mut(&mut *caller);
        data[heap_ptr..heap_ptr + bytes.len()].copy_from_slice(bytes);
        data[heap_ptr + bytes.len()] = 0;
        data[end..aligned_end].fill(0);
    }
    let _ = env.heap_ptr.set(&mut *caller, Val::I32(aligned_end as i32));
    if let Some(alloc_ptr) = env.alloc_ptr {
        let _ = alloc_ptr.set(&mut *caller, Val::I32(aligned_end as i32));
    }
    Some(heap_ptr as u32)
}

#[cfg(test)]
mod format_radix_tests {
    use super::*;

    #[test]
    fn format_f64_radix_fractional() {
        assert_eq!(format_f64_radix_to_string(255.5, 16), "ff.8");
        assert_eq!(format_f64_radix_to_string(255.0, 16), "ff");
    }
}
