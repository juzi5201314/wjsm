//! `%Object.prototype%` 按需求值方法的后端无关算法：isPrototypeOf /
//! toLocaleString 与 Annex B 的 `__proto__` 访问器对、`__defineGetter__` /
//! `__defineSetter__` / `__lookupGetter__` / `__lookupSetter__`
//! （ES §20.1.3、§B.2.2）。
//!
//! 算法经 [`ObjectProtocol`] 与宿主内部方法交互：ToObject 分类、
//! [[GetPrototypeOf]]、[[SetPrototypeOf]]、[[GetOwnProperty]] 归约与
//! Call/Get 全部由宿主适配层提供，Proxy trap、RegExp / 基元包装对象等
//! exotic 类别在协议实现内统一处理，算法本身只表达规范步骤。

use wjsm_host::Value;
use wjsm_ir::value;

/// 单层 [[GetOwnProperty]] 的归约形态：算法只关心访问器双侧、[[Enumerable]]
/// 与数据/缺失三态（`__lookupGetter__` 命中数据属性即终止，HasOwnProperty
/// 只看存在性，propertyIsEnumerable 读 enumerable）。
pub enum OwnProperty {
    /// 自有访问器属性；缺失侧以 undefined 编码。
    Accessor {
        getter: Value,
        setter: Value,
        enumerable: bool,
    },
    /// 自有数据属性。
    Data { enumerable: bool },
    /// 该层无此自有属性。
    Missing,
}

/// 统一对象协议：`%Object.prototype%` 方法所需的最小宿主内部方法集。
///
/// 所有值均为宿主 NaN-box 编码；`Err(Value)` 一律携带宿主异常值
/// （abrupt completion），由调用方原样上抛。
pub trait ObjectProtocol {
    /// 值是否为规范意义上的 Object（含数组 / callable / Proxy / RegExp）。
    fn is_object(&mut self, encoded: Value) -> bool;
    /// IsCallable（§7.2.3）：含 [[Call]] 目标可调用的 Proxy。
    fn is_callable(&mut self, encoded: Value) -> bool;
    /// 对象引用同一性（SameValue 在对象域的裁剪）。
    fn same_object(&mut self, left: Value, right: Value) -> bool;
    /// `O.[[GetPrototypeOf]]()`（§10.1.1 / §10.5.1，含 Proxy trap）。
    fn prototype_of(&mut self, object: Value) -> Result<Value, Value>;
    /// `ToObject(基元).[[GetPrototypeOf]]()`：基元映射到对应 %X.prototype%。
    fn primitive_prototype(&mut self, primitive: Value) -> Result<Value, Value>;
    /// `O.[[SetPrototypeOf]](proto)`（§10.1.2 / §10.5.2）：`Ok(false)` 表拒绝。
    fn set_prototype_of(&mut self, object: Value, prototype: Value) -> Result<bool, Value>;
    /// 单层 [[GetOwnProperty]] 归约（§10.1.5 / §10.5.5）：对基元 this 归约
    /// ToObject 包装对象的 exotic 自有层（字符串索引 / length）。
    fn own_property(&mut self, holder: Value, key: Value) -> Result<OwnProperty, Value>;
    /// ToPropertyKey（§7.1.19）：可能执行用户代码（@@toPrimitive）。
    fn to_property_key(&mut self, encoded: Value) -> Result<Value, Value>;
    /// DefinePropertyOrThrow(O, key, {get|set, enumerable: true, configurable: true})。
    fn define_accessor(
        &mut self,
        object: Value,
        key: Value,
        accessor: Value,
        is_getter: bool,
    ) -> Result<(), Value>;
    /// Get(O, P)（§7.3.2）；`name` 为已知内部方法名。
    fn get_named(&mut self, object: Value, name: &str) -> Result<Value, Value>;
    /// Call(F, V, args)（§7.3.14）。
    fn call(
        &mut self,
        callable: Value,
        this_value: Value,
        arguments: &[Value],
    ) -> Result<Value, Value>;
    /// 宿主 TypeError 异常值。
    fn type_error(&mut self, message: &str) -> Value;
    /// Invoke 命中非 callable 值时的宿主措辞（V8 风格 `number 1` 等）。
    fn describe_non_callable(&mut self, encoded: Value) -> String;
}

/// RequireObjectCoercible（§7.2.1）：null/undefined 抛 TypeError。
fn require_object_coercible<P: ObjectProtocol>(
    protocol: &mut P,
    encoded: Value,
    message: &str,
) -> Result<(), Value> {
    if value::is_null(encoded) || value::is_undefined(encoded) {
        return Err(protocol.type_error(message));
    }
    Ok(())
}

/// `get Object.prototype.__proto__`（§B.2.2.1.1）：ToObject(this) 后取
/// [[GetPrototypeOf]]；基元包装对象的原型即对应 %X.prototype%。
pub fn proto_getter<P: ObjectProtocol>(protocol: &mut P, this: Value) -> Result<Value, Value> {
    require_object_coercible(protocol, this, "Cannot convert undefined or null to object")?;
    if protocol.is_object(this) {
        return protocol.prototype_of(this);
    }
    protocol.primitive_prototype(this)
}

/// `set Object.prototype.__proto__`（§B.2.2.1.2）：proto 非对象/null 或 this
/// 为基元时静默返回 undefined；[[SetPrototypeOf]] 拒绝时抛 TypeError。
pub fn proto_setter<P: ObjectProtocol>(
    protocol: &mut P,
    this: Value,
    proto: Value,
) -> Result<Value, Value> {
    require_object_coercible(
        protocol,
        this,
        "set Object.prototype.__proto__ called on null or undefined",
    )?;
    if !value::is_null(proto) && !protocol.is_object(proto) {
        return Ok(value::encode_undefined());
    }
    if !protocol.is_object(this) {
        return Ok(value::encode_undefined());
    }
    if !protocol.set_prototype_of(this, proto)? {
        return Err(protocol.type_error("Cyclic __proto__ value"));
    }
    Ok(value::encode_undefined())
}

/// `Object.prototype.isPrototypeOf`（§20.1.3.3）：步骤 1 的非对象 V 先于
/// ToObject(this) 短路；链行走逐层经 [[GetPrototypeOf]]（Proxy trap 生效，
/// 异常原样传播）；基元 this 的临时包装对象不可能出现在任何链上。
pub fn is_prototype_of<P: ObjectProtocol>(
    protocol: &mut P,
    this: Value,
    target: Value,
) -> Result<Value, Value> {
    if !protocol.is_object(target) {
        return Ok(value::encode_bool(false));
    }
    require_object_coercible(protocol, this, "Cannot convert undefined or null to object")?;
    let this_is_object = protocol.is_object(this);
    let mut current = target;
    loop {
        current = protocol.prototype_of(current)?;
        if !protocol.is_object(current) {
            return Ok(value::encode_bool(false));
        }
        if this_is_object && protocol.same_object(current, this) {
            return Ok(value::encode_bool(true));
        }
    }
}

/// `Object.prototype.hasOwnProperty`（§20.1.3.2）：ToPropertyKey 先于
/// ToObject（副作用顺序可观测）；自有层归约覆盖 Proxy trap、RegExp
/// lastIndex、字符串索引 / length 等 exotic 类别，基元 this 归约其
/// ToObject 包装对象。
pub fn has_own_property<P: ObjectProtocol>(
    protocol: &mut P,
    this: Value,
    key: Value,
) -> Result<Value, Value> {
    let key = protocol.to_property_key(key)?;
    require_object_coercible(protocol, this, "Cannot convert undefined or null to object")?;
    Ok(value::encode_bool(!matches!(
        protocol.own_property(this, key)?,
        OwnProperty::Missing
    )))
}

/// `Object.hasOwn`（§20.1.2.13）：与 hasOwnProperty 同体，但 ToObject
/// 先于 ToPropertyKey——null/undefined 的 TypeError 先于键的副作用。
pub fn object_has_own<P: ObjectProtocol>(
    protocol: &mut P,
    target: Value,
    key: Value,
) -> Result<Value, Value> {
    require_object_coercible(
        protocol,
        target,
        "Cannot convert undefined or null to object",
    )?;
    let key = protocol.to_property_key(key)?;
    Ok(value::encode_bool(!matches!(
        protocol.own_property(target, key)?,
        OwnProperty::Missing
    )))
}

/// `Object.prototype.propertyIsEnumerable`（§20.1.3.4）：自有且
/// [[Enumerable]] 才为 true；访问器与数据属性同规则。
pub fn property_is_enumerable<P: ObjectProtocol>(
    protocol: &mut P,
    this: Value,
    key: Value,
) -> Result<Value, Value> {
    let key = protocol.to_property_key(key)?;
    require_object_coercible(protocol, this, "Cannot convert undefined or null to object")?;
    let enumerable = match protocol.own_property(this, key)? {
        OwnProperty::Accessor { enumerable, .. } | OwnProperty::Data { enumerable } => enumerable,
        OwnProperty::Missing => false,
    };
    Ok(value::encode_bool(enumerable))
}

/// `Object.prototype.toLocaleString`（§20.1.3.5）：Invoke(this, "toString")；
/// callable Proxy 经协议 IsCallable 认可。
pub fn to_locale_string<P: ObjectProtocol>(protocol: &mut P, this: Value) -> Result<Value, Value> {
    require_object_coercible(
        protocol,
        this,
        "Object.prototype.toLocaleString called on null or undefined",
    )?;
    let method = protocol.get_named(this, "toString")?;
    if !protocol.is_callable(method) {
        let description = protocol.describe_non_callable(method);
        return Err(protocol.type_error(&format!("{description} is not a function")));
    }
    protocol.call(method, this, &[])
}

/// `__defineGetter__` / `__defineSetter__`（§B.2.2.2 / §B.2.2.3）：
/// ToObject(this) 先行（null/undefined 抛错），getter/setter 非 callable 抛
/// TypeError，ToPropertyKey 的副作用总在定义前发生；基元 this 的包装对象
/// 即弃。定义经 DefinePropertyOrThrow（Proxy defineProperty trap 生效）。
pub fn define_accessor_member<P: ObjectProtocol>(
    protocol: &mut P,
    this: Value,
    key: Value,
    accessor: Value,
    is_getter: bool,
) -> Result<Value, Value> {
    require_object_coercible(protocol, this, "Cannot convert undefined or null to object")?;
    if !protocol.is_callable(accessor) {
        let message = if is_getter {
            "Object.prototype.__defineGetter__: Expecting function"
        } else {
            "Object.prototype.__defineSetter__: Expecting function"
        };
        return Err(protocol.type_error(message));
    }
    let key = protocol.to_property_key(key)?;
    if !protocol.is_object(this) {
        return Ok(value::encode_undefined());
    }
    protocol.define_accessor(this, key, accessor, is_getter)?;
    Ok(value::encode_undefined())
}

/// `__lookupGetter__` / `__lookupSetter__`（§B.2.2.4 / §B.2.2.5）：沿原型链
/// 找首个自有属性——访问器返回对应侧（可能是 undefined），数据属性（含
/// RegExp lastIndex、字符串索引等 exotic 自有属性）终止返回 undefined；
/// 链行走经 [[GetPrototypeOf]]，Proxy 层的 trap 与异常语义完整生效。
pub fn lookup_accessor_member<P: ObjectProtocol>(
    protocol: &mut P,
    this: Value,
    key: Value,
    want_getter: bool,
) -> Result<Value, Value> {
    require_object_coercible(protocol, this, "Cannot convert undefined or null to object")?;
    let key = protocol.to_property_key(key)?;
    let mut current = this;
    let mut current_is_object = protocol.is_object(this);
    loop {
        match protocol.own_property(current, key)? {
            OwnProperty::Accessor { getter, setter, .. } => {
                return Ok(if want_getter { getter } else { setter });
            }
            OwnProperty::Data { .. } => return Ok(value::encode_undefined()),
            OwnProperty::Missing => {}
        }
        current = if current_is_object {
            protocol.prototype_of(current)?
        } else {
            protocol.primitive_prototype(current)?
        };
        if !protocol.is_object(current) {
            return Ok(value::encode_undefined());
        }
        current_is_object = true;
    }
}
