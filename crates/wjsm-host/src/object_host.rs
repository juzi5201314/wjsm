//! 对象与数组分配能力。
//!
//! 后端无关的对象分配与属性访问。语义经 [`HeapContext`] 落到后端堆。

use crate::heap_context::HeapContext;
use crate::{Handle, Value};

/// 对象 / 数组分配与属性访问能力。方法接收后端上下文 `ctx`。
pub trait ObjectHost {
    /// 分配一个空对象，返回其 handle。
    fn alloc_object(&mut self, ctx: &mut dyn HeapContext) -> Handle {
        let val = ctx.alloc_object(0);
        wjsm_ir::value::decode_handle(val)
    }

    /// 分配一个指定初始容量的数组，返回其 handle。
    fn alloc_array(&mut self, ctx: &mut dyn HeapContext, capacity: u32) -> Handle {
        let val = ctx.alloc_array(capacity);
        wjsm_ir::value::decode_handle(val)
    }

    /// 读取对象属性；未定义返回 `Value` 编码的 `undefined`。
    fn get_property(&mut self, ctx: &mut dyn HeapContext, obj: Handle, key: &str) -> Value {
        ctx.get_property(obj, key)
            .unwrap_or_else(wjsm_ir::value::encode_undefined)
    }

    /// 写入对象属性。
    fn set_property(&mut self, ctx: &mut dyn HeapContext, obj: Handle, key: &str, value: Value) {
        ctx.set_property(obj, key, value);
    }

    /// 删除对象属性，返回是否成功。
    fn delete_property(&mut self, ctx: &mut dyn HeapContext, obj: Handle, key: &str) -> bool {
        ctx.delete_property(obj, key)
    }
}
