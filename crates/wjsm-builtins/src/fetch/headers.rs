use wjsm_host::{ExecContext, HeadersGuard, HeadersMethodKind, Value};
use wjsm_ir::value;

use super::objects::{
    create_empty_headers, create_headers_object, hidden_handle, init_headers_object,
};

pub fn construct<E: ExecContext>(ctx: &mut E, this_value: Value, args: &[Value]) -> Value {
    let handle = create_empty_headers(ctx);
    if let Some(init) = args.first().copied()
        && let Err(exception) = fill_from_init(ctx, handle, init)
    {
        return exception;
    }
    let object = if value::is_js_object(this_value) {
        this_value
    } else {
        ctx.alloc_object(16)
    };
    init_headers_object(ctx, object, handle);
    object
}

pub fn call_method<E: ExecContext>(
    ctx: &mut E,
    this_value: Value,
    handle: u32,
    kind: HeadersMethodKind,
    args: &[Value],
) -> Option<Value> {
    match kind {
        HeadersMethodKind::Get => {
            let name = string_from_value(ctx, *args.first()?)
                .ok()?
                .to_ascii_lowercase();
            let values = ctx.with_headers(handle, |entry| {
                entry
                    .pairs
                    .iter()
                    .filter(|(key, _)| key == &name)
                    .map(|(_, value)| value.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            })?;
            Some(if values.is_empty() {
                value::encode_null()
            } else {
                ctx.store_string_owned(values)
            })
        }
        HeadersMethodKind::Set => {
            if args.len() < 2 {
                return Some(value::encode_undefined());
            }
            let name = string_from_value(ctx, args[0]).ok()?;
            let content = string_from_value(ctx, args[1]).ok()?;
            Some(match set_pair(ctx, handle, name, content) {
                Ok(()) => value::encode_undefined(),
                Err(exception) => exception,
            })
        }
        HeadersMethodKind::Has => {
            let name = string_from_value(ctx, *args.first()?)
                .ok()?
                .to_ascii_lowercase();
            ctx.with_headers(handle, |entry| {
                value::encode_bool(entry.pairs.iter().any(|(key, _)| key == &name))
            })
        }
        HeadersMethodKind::Delete => {
            let name = string_from_value(ctx, *args.first()?)
                .ok()?
                .to_ascii_lowercase();
            ctx.with_headers(handle, |entry| {
                let before = entry.pairs.len();
                entry.pairs.retain(|(key, _)| key != &name);
                value::encode_bool(entry.pairs.len() < before)
            })
        }
        HeadersMethodKind::Append => {
            if args.len() < 2 {
                return Some(value::encode_undefined());
            }
            let name = string_from_value(ctx, args[0]).ok()?;
            let content = string_from_value(ctx, args[1]).ok()?;
            Some(match append_pair(ctx, handle, name, content) {
                Ok(()) => value::encode_undefined(),
                Err(exception) => exception,
            })
        }
        HeadersMethodKind::Keys | HeadersMethodKind::Values | HeadersMethodKind::Entries => {
            create_iterator(ctx, handle, kind)
        }
        HeadersMethodKind::ForEach => {
            let callback = args.first().copied()?;
            if !ctx.is_callable(callback) {
                return Some(value::encode_undefined());
            }
            let this_arg = args.get(1).copied().unwrap_or_else(value::encode_undefined);
            let pairs = ctx.with_headers(handle, |entry| entry.pairs.clone())?;
            for (name, content) in pairs {
                let name = ctx.store_string_owned(name);
                let content = ctx.store_string_owned(content);
                let arguments = [content, name, this_value];
                ctx.call_js(callback, this_arg, &arguments).ok()?;
            }
            Some(value::encode_undefined())
        }
    }
}

fn create_iterator<E: ExecContext>(
    ctx: &mut E,
    handle: u32,
    kind: HeadersMethodKind,
) -> Option<Value> {
    let pairs = ctx.with_headers(handle, |entry| entry.pairs.clone())?;
    let array = ctx.alloc_array(pairs.len() as u32);
    for (index, (name, content)) in pairs.into_iter().enumerate() {
        let item = match kind {
            HeadersMethodKind::Keys => ctx.store_string_owned(name),
            HeadersMethodKind::Values => ctx.store_string_owned(content),
            HeadersMethodKind::Entries => {
                let pair = ctx.alloc_array(2);
                let name = ctx.store_string_owned(name);
                let content = ctx.store_string_owned(content);
                ctx.array_write_elem(pair, 0, name);
                ctx.array_write_elem(pair, 1, content);
                pair
            }
            _ => unreachable!(),
        };
        ctx.array_write_elem(array, index as u32, item);
    }
    Some(ctx.create_array_iterator(array))
}

pub fn clone_handle<E: ExecContext>(ctx: &mut E, source: u32) -> u32 {
    let pairs = ctx
        .with_headers(source, |entry| entry.pairs.clone())
        .unwrap_or_default();
    ctx.alloc_headers(wjsm_host::HeadersEntry {
        pairs,
        guard: HeadersGuard::None,
    })
}

pub fn create_from_init<E: ExecContext>(ctx: &mut E, init: Value) -> Result<u32, Value> {
    let handle = create_empty_headers(ctx);
    fill_from_init(ctx, handle, init)?;
    Ok(handle)
}

pub fn fill_from_init<E: ExecContext>(ctx: &mut E, handle: u32, init: Value) -> Result<(), Value> {
    if value::is_undefined(init) || value::is_null(init) {
        return Ok(());
    }
    if let Some(source) = hidden_handle(ctx, init, "__headers_handle__") {
        let pairs = ctx
            .with_headers(source, |entry| entry.pairs.clone())
            .unwrap_or_default();
        let _ = ctx.with_headers(handle, |entry| entry.pairs.extend(pairs));
        return Ok(());
    }
    if value::is_array(init) {
        let length = ctx
            .array_read_length(init)
            .ok_or_else(|| ctx.make_type_error("invalid Headers init"))?;
        for index in 0..length {
            let entry = ctx
                .array_read_elem(init, index)
                .unwrap_or_else(value::encode_undefined);
            if !value::is_array(entry) || ctx.array_read_length(entry) != Some(2) {
                return Err(ctx.make_type_error("Headers sequence entry must have length 2"));
            }
            let name = ctx
                .array_read_elem(entry, 0)
                .unwrap_or_else(value::encode_undefined);
            let content = ctx
                .array_read_elem(entry, 1)
                .unwrap_or_else(value::encode_undefined);
            let name = string_from_value(ctx, name)?;
            let content = string_from_value(ctx, content)?;
            append_pair(ctx, handle, name, content)?;
        }
        return Ok(());
    }
    if value::is_js_object(init) {
        let enumerator = ctx.create_enumerator(init);
        let enumerator_handle = value::decode_handle(enumerator);
        while !ctx.enumerator_done(enumerator_handle) {
            let key = ctx.enumerator_key(enumerator_handle);
            let key_string = ctx.read_string_utf8_lossy(key);
            let raw = ctx.read_property_by_string_key(init, &key_string);
            let content = string_from_value(ctx, raw)?;
            set_pair(ctx, handle, key_string, content)?;
            ctx.enumerator_advance(enumerator_handle);
        }
    }
    Ok(())
}

fn append_pair<E: ExecContext>(
    ctx: &mut E,
    handle: u32,
    name: String,
    content: String,
) -> Result<(), Value> {
    validate_pair(ctx, &name, &content)?;
    let _ = ctx.with_headers(handle, |entry| {
        entry.pairs.push((name.to_ascii_lowercase(), content));
    });
    Ok(())
}

fn set_pair<E: ExecContext>(
    ctx: &mut E,
    handle: u32,
    name: String,
    content: String,
) -> Result<(), Value> {
    validate_pair(ctx, &name, &content)?;
    let lower = name.to_ascii_lowercase();
    let _ = ctx.with_headers(handle, |entry| {
        entry.pairs.retain(|(key, _)| key != &lower);
        entry.pairs.push((lower, content));
    });
    Ok(())
}

fn validate_pair<E: ExecContext>(ctx: &mut E, name: &str, content: &str) -> Result<(), Value> {
    if name.is_empty()
        || !name.as_bytes().iter().all(|byte| {
            matches!(
                *byte,
                b'0'..=b'9'
                    | b'A'..=b'Z'
                    | b'a'..=b'z'
                    | b'!'
                    | b'#'
                    | b'$'
                    | b'%'
                    | b'&'
                    | b'\''
                    | b'*'
                    | b'+'
                    | b'-'
                    | b'.'
                    | b'^'
                    | b'_'
                    | b'`'
                    | b'|'
                    | b'~'
            )
        })
    {
        return Err(ctx.make_type_error("invalid header name"));
    }
    if content
        .bytes()
        .any(|byte| matches!(byte, b'\r' | b'\n' | 0))
    {
        return Err(ctx.make_type_error("invalid header value"));
    }
    Ok(())
}

pub fn string_from_value<E: ExecContext>(ctx: &mut E, raw: Value) -> Result<String, Value> {
    crate::json::json_parse_to_string_impl(ctx, raw)
}

pub fn object_headers_handle<E: ExecContext>(ctx: &mut E, object: Value) -> Option<u32> {
    hidden_handle(ctx, object, "__headers_handle__")
}

pub fn clone_object<E: ExecContext>(ctx: &mut E, source: u32) -> Value {
    let handle = clone_handle(ctx, source);
    create_headers_object(ctx, handle)
}
