use super::*;

/// 对象解构 RequireObjectCoercible 检查（§8.6.2 BindingInitialization 步骤 1 /
/// §13.15.5.2 步骤 1）的调用点文本。V8 的文案由属性加载失败时重新解析源码
/// （CallPrinter）得出，本引擎在 lowering 期按 AST 预计算等价文本。
#[derive(Clone, Debug)]
pub(crate) enum DestructureCallsite {
    /// 源码可渲染的表达式文本（`o` / `null` / `o.f(...)` / `.for` / `.catch`）。
    Text(String),
    /// 函数形参：无源码文本，按运行期值渲染（V8 BuildDefaultCallSite 的
    /// typeof 前缀：null → "object null"，undefined → "undefined"）。
    RuntimeDefault,
}

impl DestructureCallsite {
    /// null 分支的调用点文本。
    fn for_null(&self) -> &str {
        match self {
            Self::Text(text) => text,
            Self::RuntimeDefault => "object null",
        }
    }

    /// undefined 分支的调用点文本。
    fn for_undefined(&self) -> &str {
        match self {
            Self::Text(text) => text,
            Self::RuntimeDefault => "undefined",
        }
    }
}

/// 对象解构模式的值来源上下文，决定 coercible 检查的发射与文案形态
/// （与 Node/V8 逐字节对拍的经验矩阵，见 `emit_object_coercible_check`）。
#[derive(Clone, Debug)]
pub(crate) enum DestructureSource {
    /// 顶层：声明初始化器 / 解构赋值 RHS / 解构默认值表达式 / for-of 迭代值
    /// （`.for`）/ catch 形参（`.catch`）/ 函数形参（RuntimeDefault）。
    TopLevel(DestructureCallsite),
    /// 对象模式属性值位置的嵌套模式：携带外层属性键（计算键为 None）与
    /// 顶层调用点（数组元素下的嵌套无调用点，为 None）。
    NestedInObject {
        outer_key: Option<String>,
        callsite: Option<DestructureCallsite>,
    },
    /// 数组模式元素位置的嵌套模式：V8 对此处的检查降级为普通读取文案。
    NestedInArray,
}

/// 对象模式首属性的形态分类，决定检查是否发射与文案模板。
enum FirstProp {
    /// 非计算键且无默认值：nullish 时首个 GetProp 必然抛错，顶层升级为
    /// 「Cannot destructure property '<key>' ...」文案。
    Simple(String),
    /// 非计算键且有默认值（`{a = 1}` / `{"x": a = 1}`）：V8 不发检查，
    /// 由 GetProp 的读取 TypeError 承担（文案即普通读取错误）。
    HasDefault,
    /// 空模式 / 计算键在先 / rest 在先：无先行 GetProp 或键求值有副作用，
    /// 必须显式检查（RequireObjectCoercible 先于键求值）。bool 为 rest 在先
    /// （V8 嵌套位置对 rest 在先降级为普通读取文案）。
    NeedsCheck(bool),
}

/// 非计算属性键的文本（计算键返回 None）。
pub(crate) fn render_prop_name(key: &swc_ast::PropName) -> Option<String> {
    match key {
        swc_ast::PropName::Ident(ident) => Some(ident.sym.to_string()),
        swc_ast::PropName::Str(text) => Some(text.value.to_string_lossy().into_owned()),
        swc_ast::PropName::Num(num) => Some(js_number_property_key(num.value)),
        swc_ast::PropName::BigInt(bigint) => Some(bigint.value.to_string()),
        swc_ast::PropName::Computed(_) => None,
    }
}

/// 对象模式首属性分类。
fn classify_first_prop(object_pat: &swc_ast::ObjectPat) -> FirstProp {
    let Some(first) = object_pat.props.first() else {
        return FirstProp::NeedsCheck(false);
    };
    match first {
        swc_ast::ObjectPatProp::Rest(_) => FirstProp::NeedsCheck(true),
        swc_ast::ObjectPatProp::Assign(assign) => {
            if assign.value.is_some() {
                FirstProp::HasDefault
            } else {
                FirstProp::Simple(assign.key.id.sym.to_string())
            }
        }
        swc_ast::ObjectPatProp::KeyValue(kv) => match render_prop_name(&kv.key) {
            None => FirstProp::NeedsCheck(false),
            Some(key) => {
                if matches!(&*kv.value, swc_ast::Pat::Assign(_)) {
                    FirstProp::HasDefault
                } else {
                    FirstProp::Simple(key)
                }
            }
        },
    }
}

/// CallPrinter 风格的解构 RHS 表达式文本（V8 经验子集）：标识符原名、null
/// 字面量、成员链（点号 / 字符串字面量键点化 / 数字字面量键下标）、调用
/// `callee(...)`、括号透明、二元运算加括号、条件表达式三连
/// "(intermediate value)"、对象字面量按属性数重复；其余形态回退单个
/// "(intermediate value)"（V8 对少数复杂形态渲染更精细，属可接受偏离）。
pub(crate) fn render_destructure_callsite(expr: &swc_ast::Expr) -> String {
    const INTERMEDIATE: &str = "(intermediate value)";
    match expr {
        swc_ast::Expr::Ident(ident) => ident.sym.to_string(),
        swc_ast::Expr::This(_) => "this".into(),
        swc_ast::Expr::Lit(swc_ast::Lit::Null(_)) => "null".into(),
        swc_ast::Expr::Lit(swc_ast::Lit::Num(num)) => js_number_property_key(num.value),
        swc_ast::Expr::Paren(paren) => render_destructure_callsite(&paren.expr),
        swc_ast::Expr::Member(member) => {
            let base = render_destructure_callsite(&member.obj);
            match &member.prop {
                swc_ast::MemberProp::Ident(ident) => format!("{base}.{}", ident.sym),
                swc_ast::MemberProp::Computed(computed) => match &*computed.expr {
                    swc_ast::Expr::Lit(swc_ast::Lit::Str(text)) => {
                        format!("{base}.{}", text.value.to_string_lossy())
                    }
                    swc_ast::Expr::Lit(swc_ast::Lit::Num(num)) => {
                        format!("{base}[{}]", js_number_property_key(num.value))
                    }
                    _ => INTERMEDIATE.into(),
                },
                swc_ast::MemberProp::PrivateName(name) => format!("{base}.#{}", name.name),
            }
        }
        swc_ast::Expr::Call(call) => match &call.callee {
            swc_ast::Callee::Expr(callee) => {
                format!("{}(...)", render_destructure_callsite(callee))
            }
            _ => INTERMEDIATE.into(),
        },
        swc_ast::Expr::Bin(bin) => format!(
            "({} {} {})",
            render_destructure_callsite(&bin.left),
            bin.op.as_str(),
            render_destructure_callsite(&bin.right),
        ),
        swc_ast::Expr::Cond(_) => INTERMEDIATE.repeat(3),
        swc_ast::Expr::Object(object) => {
            format!("{{{}}}", INTERMEDIATE.repeat(object.props.len()))
        }
        _ => INTERMEDIATE.into(),
    }
}

impl Lowerer {
    /// 对象解构的 RequireObjectCoercible 检查（对拍 Node v22 的经验矩阵）：
    ///
    /// | 来源 \ 首属性 | Simple(k) | HasDefault | 空/计算键 | rest 在先 |
    /// |---|---|---|---|---|
    /// | 顶层(cs) | destructure property 'k' of 'cs' | 不发检查 | destructure 'cs' | destructure 'cs' |
    /// | 嵌套于对象(ok, cs) | 不发检查 | 不发检查 | destructure property 'ok' of 'cs' | 读取无键文案 |
    /// | 嵌套于数组 | 不发检查 | 不发检查 | 读取无键文案 | 读取无键文案 |
    ///
    /// 不发检查的格子由首个 GetProp 的 ToObject TypeError 承担（读取文案），
    /// 与 V8 消除冗余检查的策略一致；发检查的格子先于任何键求值 / 属性读取
    /// （RequireObjectCoercible 在 PropertyBindingInitialization 之前）。
    ///
    /// 返回（延续块，KV 子模式应继承的调用点）。
    pub(crate) fn emit_object_coercible_check(
        &mut self,
        object_pat: &swc_ast::ObjectPat,
        src_val: ValueId,
        block: BasicBlockId,
        source: &DestructureSource,
    ) -> Result<(BasicBlockId, Option<DestructureCallsite>), LoweringError> {
        let child_callsite = match source {
            DestructureSource::TopLevel(callsite) => Some(callsite.clone()),
            DestructureSource::NestedInObject { callsite, .. } => callsite.clone(),
            DestructureSource::NestedInArray => None,
        };
        let messages = match (source, classify_first_prop(object_pat)) {
            (_, FirstProp::HasDefault) => None,
            (DestructureSource::TopLevel(cs), FirstProp::Simple(key)) => Some((
                format!(
                    "Cannot destructure property '{key}' of '{}' as it is null.",
                    cs.for_null()
                ),
                format!(
                    "Cannot destructure property '{key}' of '{}' as it is undefined.",
                    cs.for_undefined()
                ),
            )),
            (DestructureSource::TopLevel(cs), FirstProp::NeedsCheck(_)) => Some((
                format!("Cannot destructure '{}' as it is null.", cs.for_null()),
                format!(
                    "Cannot destructure '{}' as it is undefined.",
                    cs.for_undefined()
                ),
            )),
            (
                DestructureSource::NestedInObject {
                    outer_key: Some(outer_key),
                    callsite: Some(cs),
                },
                FirstProp::NeedsCheck(false),
            ) => Some((
                format!(
                    "Cannot destructure property '{outer_key}' of '{}' as it is null.",
                    cs.for_null()
                ),
                format!(
                    "Cannot destructure property '{outer_key}' of '{}' as it is undefined.",
                    cs.for_undefined()
                ),
            )),
            (
                DestructureSource::NestedInObject {
                    outer_key: None,
                    callsite: Some(cs),
                },
                FirstProp::NeedsCheck(false),
            ) => Some((
                format!("Cannot destructure '{}' as it is null.", cs.for_null()),
                format!(
                    "Cannot destructure '{}' as it is undefined.",
                    cs.for_undefined()
                ),
            )),
            // 数组元素下的嵌套、无调用点、以及嵌套位置的 rest 在先：V8 的
            // 位置回溯不识别为解构，落到普通读取的无键文案。
            (
                DestructureSource::NestedInObject { .. } | DestructureSource::NestedInArray,
                FirstProp::NeedsCheck(_),
            ) => Some((
                "Cannot read properties of null".into(),
                "Cannot read properties of undefined".into(),
            )),
            (
                DestructureSource::NestedInObject { .. } | DestructureSource::NestedInArray,
                FirstProp::Simple(_),
            ) => None,
        };
        let Some((msg_null, msg_undefined)) = messages else {
            return Ok((block, child_callsite));
        };
        let block = self.emit_destructure_nullish_throw(src_val, block, msg_null, msg_undefined)?;
        Ok((block, child_callsite))
    }

    /// 发射 `if (v === null) throw TypeError(msg_null); if (v === undefined)
    /// throw TypeError(msg_undefined);`。两个分支的文案在编译期定死（null /
    /// undefined 各一），throw 经 emit_throw_value 走既有 abrupt 展开（迭代器
    /// 保护区 close、finally 等）。返回非 nullish 的延续块。
    fn emit_destructure_nullish_throw(
        &mut self,
        src_val: ValueId,
        block: BasicBlockId,
        msg_null: String,
        msg_undefined: String,
    ) -> Result<BasicBlockId, LoweringError> {
        let mut current_block = block;
        for (constant, message) in [
            (Constant::Null, msg_null),
            (Constant::Undefined, msg_undefined),
        ] {
            let sentinel_const = self.module.add_constant(constant);
            let sentinel_val = self.alloc_value();
            self.current_function.append_instruction(
                current_block,
                Instruction::Const {
                    dest: sentinel_val,
                    constant: sentinel_const,
                },
            );
            let matched = self.alloc_value();
            self.current_function.append_instruction(
                current_block,
                Instruction::Compare {
                    dest: matched,
                    op: CompareOp::StrictEq,
                    lhs: src_val,
                    rhs: sentinel_val,
                },
            );
            let throw_block = self.current_function.new_block();
            let next_block = self.current_function.new_block();
            self.current_function.set_terminator(
                current_block,
                Terminator::Branch {
                    condition: matched,
                    true_block: throw_block,
                    false_block: next_block,
                },
            );
            let msg_const = self.module.add_constant(Constant::String(message));
            let msg_val = self.alloc_value();
            self.current_function.append_instruction(
                throw_block,
                Instruction::Const {
                    dest: msg_val,
                    constant: msg_const,
                },
            );
            let error_val = self.alloc_value();
            self.current_function.append_instruction(
                throw_block,
                Instruction::CallBuiltin {
                    dest: Some(error_val),
                    builtin: Builtin::TypeErrorConstructor,
                    args: vec![msg_val],
                },
            );
            self.emit_throw_value(throw_block, error_val)?;
            current_block = next_block;
        }
        Ok(current_block)
    }
}
