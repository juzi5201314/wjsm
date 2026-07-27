//! Console 宿主能力。
//!
//! 对应 ECMAScript host 环境的 `console.*` 输出。语义实现基于 [`HeapContext`]
//! 的后端无关操作，各后端只需提供 `HeapContext` 即可获得完整 console 行为。

use crate::Value;
use crate::heap_context::HeapContext;

/// `console.*` 输出能力。方法接收后端上下文 `ctx`。
pub trait ConsoleHost {
    /// `console.log(...args)` → stdout。
    ///
    /// 默认实现经 `ctx` 渲染各值并写入输出缓冲；可覆盖以定制。
    fn console_log(&mut self, ctx: &mut dyn HeapContext, args: &[Value]) -> anyhow::Result<()> {
        write_console_line(ctx, args, None)
    }

    /// `console.error(...args)` → stderr（带 `[error]` 前缀）。
    fn console_error(&mut self, ctx: &mut dyn HeapContext, args: &[Value]) -> anyhow::Result<()> {
        write_console_line(ctx, args, Some("error"))
    }

    /// `console.warn(...args)`（默认转发 `console_error`）。
    fn console_warn(&mut self, ctx: &mut dyn HeapContext, args: &[Value]) -> anyhow::Result<()> {
        write_console_line(ctx, args, Some("warn"))
    }

    /// `console.info(...args)`（默认同 `console_log`）。
    fn console_info(&mut self, ctx: &mut dyn HeapContext, args: &[Value]) -> anyhow::Result<()> {
        write_console_line(ctx, args, Some("info"))
    }
}

/// 渲染一组值并写入一行输出。
fn write_console_line(
    ctx: &mut dyn HeapContext,
    args: &[Value],
    prefix: Option<&str>,
) -> anyhow::Result<()> {
    let rendered: Vec<String> = args.iter().map(|v| render_console_value(ctx, *v)).collect();
    let line = rendered.join(" ");
    let mut out = Vec::with_capacity(line.len() + 16);
    if let Some(p) = prefix {
        out.extend_from_slice(format!("[{p}] ").as_bytes());
    }
    out.extend_from_slice(line.as_bytes());
    out.push(b'\n');
    ctx.write_output(&out);
    Ok(())
}

/// 把单个值渲染为 console 文本（后端无关，经 HeapContext 读堆）。
fn render_console_value(ctx: &mut dyn HeapContext, val: Value) -> String {
    if wjsm_ir::value::is_string(val) {
        return ctx.read_string_utf8(val);
    }
    if wjsm_ir::value::is_undefined(val) {
        return "undefined".to_string();
    }
    if wjsm_ir::value::is_null(val) {
        return "null".to_string();
    }
    if wjsm_ir::value::is_bool(val) {
        return if wjsm_ir::value::decode_bool(val) {
            "true"
        } else {
            "false"
        }
        .to_string();
    }
    if wjsm_ir::value::is_array(val) {
        let handle = wjsm_ir::value::decode_handle(val);
        if let Some(len) = ctx.array_length(handle) {
            let mut parts = Vec::with_capacity(len as usize);
            for i in 0..len {
                parts.push(match ctx.array_elem(handle, i) {
                    Some(elem) => render_console_value(ctx, elem),
                    None => "?".to_string(),
                });
            }
            return format!("[{}]", parts.join(", "));
        }
    }
    // 数字（raw f64 落在此）与其余 tag 的通用回退。
    if !wjsm_ir::value::is_object(val) {
        let f = wjsm_ir::value::decode_f64(val);
        return format!("{f}");
    }
    format!("[object:{}]", wjsm_ir::value::decode_handle(val))
}
