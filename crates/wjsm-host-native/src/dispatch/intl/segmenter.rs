//! `Intl.Segmenter`、Segments 与迭代器。

use wjsm_builtins::intl::resolve_locale;
use wjsm_intl_data::{OwnedSegmenter, SegmentGranularity};
use wjsm_ir::{value, wk_symbol};
use wjsm_native_abi::NativeVmContext;

use super::common::{create_instance, intern, slot_handle, throw_intl};
use super::install::{install_method, install_to_string_tag};
use super::js::{
    canonicalize_locales, get_option_string, get_options_object, supported_locales_of,
};
use super::slots::{IntlSlot, SegmentIterSlot, SegmenterSlot, SegmentsSlot};
use super::{IntlCallable, incompatible};
use crate::dispatch::runtime::{fail_dispatch, range_error, to_number_coerced, to_string_coerced};
use crate::{NativeAgentState, NativeCallableKind, PropertyKey};

pub(super) fn call(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    callable: IntlCallable,
    receiver: i64,
    args: &[i64],
) -> i64 {
    match callable {
        IntlCallable::SegmenterConstructor => construct(ctx, state, receiver, args),
        IntlCallable::SegmenterSupportedLocalesOf => supported_locales_of(ctx, state, args),
        IntlCallable::SegmenterResolvedOptions => resolved(ctx, state, receiver),
        IntlCallable::SegmenterSegment => segment(ctx, state, receiver, args),
        IntlCallable::SegmentsContaining => containing(ctx, state, receiver, args),
        IntlCallable::SegmentsIterator => iterator(ctx, state, receiver),
        IntlCallable::SegmentIteratorNext => next(ctx, state, receiver),
        _ => fail_dispatch(ctx),
    }
}

fn construct(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    this_value: i64,
    args: &[i64],
) -> i64 {
    if let Err(exception) = super::common::require_new(ctx, state) {
        return exception;
    }
    let requested = match canonicalize_locales(
        ctx,
        state,
        args.first()
            .copied()
            .unwrap_or_else(value::encode_undefined),
    ) {
        Ok(requested) => requested,
        Err(exception) => return exception,
    };
    let options = match get_options_object(
        ctx,
        state,
        args.get(1).copied().unwrap_or_else(value::encode_undefined),
    ) {
        Ok(options) => options,
        Err(exception) => return exception,
    };
    if let Err(exception) = get_option_string(
        ctx,
        state,
        options,
        "localeMatcher",
        &["lookup", "best fit"],
        Some("best fit"),
    ) {
        return exception;
    }
    let granularity = match get_option_string(
        ctx,
        state,
        options,
        "granularity",
        &["grapheme", "word", "sentence"],
        Some("grapheme"),
    ) {
        Ok(value) => value.unwrap_or_else(|| "grapheme".into()),
        Err(exception) => return exception,
    };
    let resolved = match resolve_locale(&requested, &[], &Default::default()) {
        Ok(resolved) => resolved,
        Err(error) => return throw_intl(ctx, state, error),
    };
    let kind = match granularity.as_str() {
        "word" => SegmentGranularity::Word,
        "sentence" => SegmentGranularity::Sentence,
        _ => SegmentGranularity::Grapheme,
    };
    create_instance(
        ctx,
        state,
        IntlCallable::SegmenterConstructor,
        IntlSlot::Segmenter(SegmenterSlot {
            locale: resolved.locale.clone(),
            granularity,
            formatter: match OwnedSegmenter::try_new(&resolved.locale, kind) {
                Ok(formatter) => formatter,
                Err(error) => return range_error(ctx, state, &error),
            },
        }),
        this_value,
    )
}

fn resolved(ctx: &mut NativeVmContext, state: &mut NativeAgentState, receiver: i64) -> i64 {
    let Some(handle) = slot_handle(receiver) else {
        return incompatible(ctx, state);
    };
    let Some(IntlSlot::Segmenter(slot)) = state.intl.slots.get(&handle) else {
        return incompatible(ctx, state);
    };
    let pairs = [
        ("locale", slot.locale.clone()),
        ("granularity", slot.granularity.clone()),
    ];
    super::common::resolved_strings(ctx, state, &pairs)
}

fn segment(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    receiver: i64,
    args: &[i64],
) -> i64 {
    let Some(handle) = slot_handle(receiver) else {
        return incompatible(ctx, state);
    };
    let formatter = match state.intl.slots.get(&handle) {
        Some(IntlSlot::Segmenter(slot)) => slot.formatter.clone(),
        _ => return incompatible(ctx, state),
    };
    let granularity = match state.intl.slots.get(&handle) {
        Some(IntlSlot::Segmenter(slot)) => slot.granularity.clone(),
        _ => return incompatible(ctx, state),
    };
    let input = args
        .first()
        .copied()
        .unwrap_or_else(value::encode_undefined);
    let text = match to_string_coerced(ctx, state, input) {
        Ok(text) => text,
        Err(exception) => return exception,
    };
    let segments = formatter.segments_utf16(&text);
    let breaks = formatter.break_utf16_offsets(&text);
    let word_likes = segments.iter().map(|segment| segment.word_like).collect();
    let prototype = match ensure_segments_prototype(state) {
        Some(prototype) => prototype,
        None => return fail_dispatch(ctx),
    };
    let object =
        match state.allocate_object_with_prototype(0, false, value::decode_handle(prototype)) {
            Ok(object) => object,
            Err(_) => return fail_dispatch(ctx),
        };
    state.intl.slots.insert(
        value::decode_handle(object),
        IntlSlot::Segments(SegmentsSlot {
            text,
            granularity,
            breaks,
            word_likes,
        }),
    );
    object
}

fn containing(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    receiver: i64,
    args: &[i64],
) -> i64 {
    let Some(handle) = slot_handle(receiver) else {
        return incompatible(ctx, state);
    };
    let (text, granularity, breaks, word_likes) = match state.intl.slots.get(&handle) {
        Some(IntlSlot::Segments(slot)) => (
            slot.text.clone(),
            slot.granularity.clone(),
            slot.breaks.clone(),
            slot.word_likes.clone(),
        ),
        _ => return incompatible(ctx, state),
    };
    let index = args
        .first()
        .copied()
        .unwrap_or_else(value::encode_undefined);
    let index = match to_number_coerced(ctx, state, index) {
        Ok(index) => index,
        Err(exception) => return exception,
    };
    // ToIntegerOrInfinity：NaN / ±0 → 0。
    let index = if !index.is_finite() {
        if index.is_nan() {
            0
        } else if index.is_sign_negative() {
            return value::encode_undefined();
        } else {
            i64::MAX
        }
    } else if index == 0.0 {
        0
    } else {
        index.trunc() as i64
    };
    let utf16_len = text.encode_utf16().count() as i64;
    if index < 0 || index >= utf16_len {
        return value::encode_undefined();
    }
    let index = index as u32;
    let (start, end) = segment_at(&breaks, index, utf16_len as u32);
    let word_like = word_like_at(&breaks, &word_likes, start);
    segment_object(
        ctx,
        state,
        &text,
        start,
        end,
        (granularity == "word").then_some(word_like),
    )
}

fn iterator(ctx: &mut NativeVmContext, state: &mut NativeAgentState, receiver: i64) -> i64 {
    let Some(handle) = slot_handle(receiver) else {
        return incompatible(ctx, state);
    };
    let (text, granularity, breaks, word_likes) = match state.intl.slots.get(&handle) {
        Some(IntlSlot::Segments(slot)) => (
            slot.text.clone(),
            slot.granularity.clone(),
            slot.breaks.clone(),
            slot.word_likes.clone(),
        ),
        _ => return incompatible(ctx, state),
    };
    let prototype = match ensure_iterator_prototype(state) {
        Some(prototype) => prototype,
        None => return fail_dispatch(ctx),
    };
    let object =
        match state.allocate_object_with_prototype(0, false, value::decode_handle(prototype)) {
            Ok(object) => object,
            Err(_) => return fail_dispatch(ctx),
        };
    state.intl.slots.insert(
        value::decode_handle(object),
        IntlSlot::SegmentIterator(SegmentIterSlot {
            text,
            granularity,
            breaks,
            word_likes,
            index: 0,
        }),
    );
    object
}

fn next(ctx: &mut NativeVmContext, state: &mut NativeAgentState, receiver: i64) -> i64 {
    let Some(handle) = slot_handle(receiver) else {
        return incompatible(ctx, state);
    };
    let (done, text, granularity, start, end, word_like) = match state.intl.slots.get_mut(&handle) {
        Some(IntlSlot::SegmentIterator(slot)) => {
            let utf16_len = slot.text.encode_utf16().count() as u32;
            if slot.index + 1 >= slot.breaks.len() {
                (true, String::new(), String::new(), 0, 0, false)
            } else {
                let start = slot.breaks[slot.index];
                let end = slot
                    .breaks
                    .get(slot.index + 1)
                    .copied()
                    .unwrap_or(utf16_len);
                let word_like = slot.word_likes.get(slot.index).copied().unwrap_or(false);
                slot.index += 1;
                (
                    false,
                    slot.text.clone(),
                    slot.granularity.clone(),
                    start,
                    end,
                    word_like,
                )
            }
        }
        _ => return incompatible(ctx, state),
    };
    if done {
        return iterator_result(ctx, state, value::encode_undefined(), true);
    }
    let segment = segment_object(
        ctx,
        state,
        &text,
        start,
        end,
        (granularity == "word").then_some(word_like),
    );
    if value::is_exception(segment) {
        return segment;
    }
    iterator_result(ctx, state, segment, false)
}

fn segment_at(breaks: &[u32], index: u32, utf16_len: u32) -> (u32, u32) {
    let mut start = 0;
    for window in breaks.windows(2) {
        if index >= window[0] && index < window[1] {
            return (window[0], window[1]);
        }
        start = window[0];
    }
    (start, utf16_len)
}

fn word_like_at(breaks: &[u32], word_likes: &[bool], start: u32) -> bool {
    breaks
        .iter()
        .position(|offset| *offset == start)
        .and_then(|index| word_likes.get(index).copied())
        .unwrap_or(false)
}

fn segment_object(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    text: &str,
    start: u32,
    end: u32,
    word_like: Option<bool>,
) -> i64 {
    let units: Vec<u16> = text.encode_utf16().collect();
    let start_i = start as usize;
    let end_i = (end as usize).min(units.len());
    let slice = units.get(start_i..end_i).unwrap_or(&[]);
    let segment = String::from_utf16_lossy(slice);
    let object = match state.allocate_object_with_gc_retry(ctx, 4, false) {
        Ok(object) => object,
        Err(_) => return fail_dispatch(ctx),
    };
    let segment = intern(ctx, state, segment);
    let input = intern(ctx, state, text.to_owned());
    for (name, stored) in [
        ("segment", segment),
        ("index", value::encode_f64(start as f64)),
        ("input", input),
    ] {
        if let Err(exception) = super::common::set_data(ctx, state, object, name, stored) {
            return exception;
        }
    }
    if let Some(word_like) = word_like
        && let Err(exception) = super::common::set_data(
            ctx,
            state,
            object,
            "isWordLike",
            value::encode_bool(word_like),
        )
    {
        return exception;
    }
    object
}

fn iterator_result(
    ctx: &mut NativeVmContext,
    state: &mut NativeAgentState,
    value: i64,
    done: bool,
) -> i64 {
    let object = match state.allocate_object_with_gc_retry(ctx, 2, false) {
        Ok(object) => object,
        Err(_) => return fail_dispatch(ctx),
    };
    if let Err(exception) = super::common::set_data(ctx, state, object, "value", value) {
        return exception;
    }
    if let Err(exception) =
        super::common::set_data(ctx, state, object, "done", value::encode_bool(done))
    {
        return exception;
    }
    object
}

fn ensure_segments_prototype(state: &mut NativeAgentState) -> Option<i64> {
    if let Some(prototype) = state.intl.segments_prototype {
        return Some(prototype);
    }
    let prototype = state.allocate_object(2, false).ok()?;
    install_to_string_tag(state, prototype, "Intl.Segments").ok()?;
    install_method(
        state,
        prototype,
        "containing",
        IntlCallable::SegmentsContaining,
    )
    .ok()?;
    let iterator =
        state.native_callable(NativeCallableKind::Intl(IntlCallable::SegmentsIterator))?;
    let key = PropertyKey::symbol(wk_symbol::ITERATOR);
    state
        .gc
        .heap()
        .define_data_property(
            value::decode_handle(prototype),
            key,
            iterator as u64,
            crate::BUILTIN_PROTOTYPE_PROPERTY_FLAGS,
        )
        .ok()?;
    state.intl.segments_prototype = Some(prototype);
    Some(prototype)
}

fn ensure_iterator_prototype(state: &mut NativeAgentState) -> Option<i64> {
    if let Some(prototype) = state.intl.segment_iterator_prototype {
        return Some(prototype);
    }
    let prototype = state.allocate_object(1, false).ok()?;
    install_to_string_tag(state, prototype, "Segmenter String Iterator").ok()?;
    install_method(state, prototype, "next", IntlCallable::SegmentIteratorNext).ok()?;
    state.intl.segment_iterator_prototype = Some(prototype);
    Some(prototype)
}
