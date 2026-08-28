//! 函数 `name` / `length` 元数据（SetFunctionName / SetFunctionLength）。
//!
//! 内部 IR 函数名（`C.constructor`、`arrow_N`、`$0.x` 等）只用于诊断与
//! 唯一标识；JS 可见的 `name` / `length` 属性由本模块的辅助函数按
//! ES §10.2.9 / §10.2.10 计算并写入 [`Function`] 的 js 元数据。静态可知的
//! 键在降级期确定；计算键在 ToPropertyKey 之后经 [`Builtin::FunctionSetName`]
//! 运行时设置。

use super::*;

/// 访问器前缀编码（与宿主 `FunctionSetName` handler 的约定一致）。
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum AccessorPrefix {
    None,
    Get,
    Set,
}

impl AccessorPrefix {
    pub(crate) fn wire_code(self) -> f64 {
        match self {
            Self::None => 0.0,
            Self::Get => 1.0,
            Self::Set => 2.0,
        }
    }

    /// 静态键的 `name` 字符串前缀（`get x` / `set x`）。
    pub(crate) fn apply(self, name: &str) -> String {
        match self {
            Self::None => name.to_string(),
            Self::Get => format!("get {name}"),
            Self::Set => format!("set {name}"),
        }
    }
}

impl Lowerer {
    /// ExpectedArgumentCount（ES FormalParameterList 静态语义）：首个带默认值
    /// 或 rest 的形参之前的形参个数。解构形参无初始化器时照常计数。
    pub(crate) fn expected_argument_count<'a>(
        pats: impl IntoIterator<Item = &'a swc_ast::Pat>,
    ) -> u32 {
        let mut count = 0u32;
        for pat in pats {
            match pat {
                swc_ast::Pat::Rest(_) | swc_ast::Pat::Assign(_) => break,
                _ => count += 1,
            }
        }
        count
    }

    /// [`Self::expected_argument_count`] 的 `Param` 切片便捷形态。
    pub(crate) fn expected_param_count(params: &[swc_ast::Param]) -> u32 {
        Self::expected_argument_count(params.iter().map(|param| &param.pat))
    }

    /// 静态可知的属性键名（方法 / 访问器 SetFunctionName 用）。
    /// 数字 / BigInt / 计算键返回 None——它们的键值在降级期以运行时 value
    /// 存在，`name` 统一走 [`Self::emit_runtime_set_function_name`]，与实际
    /// 属性键严格一致。
    pub(crate) fn static_prop_js_name(key: &swc_ast::PropName) -> Option<String> {
        match key {
            swc_ast::PropName::Ident(ident) => Some(ident.sym.to_string()),
            swc_ast::PropName::Str(text) => Some(text.value.to_string_lossy().into_owned()),
            swc_ast::PropName::Num(_)
            | swc_ast::PropName::BigInt(_)
            | swc_ast::PropName::Computed(_) => None,
        }
    }

    /// 静态 PropName 的键字符串（与 `lower_prop_name` 发射的 Const 完全一致，
    /// 含数字 / BigInt 键的字符串化）；计算键返回 None。用于属性值 / 字段
    /// 初始化器的 NamedEvaluation：`name` 必须等于实际属性键。
    pub(crate) fn static_prop_name_text(key: &swc_ast::PropName) -> Option<String> {
        match key {
            swc_ast::PropName::Ident(ident) => Some(ident.sym.to_string()),
            swc_ast::PropName::Str(text) => Some(text.value.to_string_lossy().into_owned()),
            swc_ast::PropName::Num(num) => Some(
                num.raw
                    .as_ref()
                    .map(|raw| raw.to_string())
                    .unwrap_or_else(|| js_number_property_key(num.value)),
            ),
            swc_ast::PropName::BigInt(bigint) => Some(bigint.value.to_string()),
            swc_ast::PropName::Computed(_) => None,
        }
    }

    /// 绑定形态的 NamedEvaluation（变量声明初始化器、形参 / 解构默认值）：
    /// 目标为标识符且右值为匿名函数定义时暂存名字提示，由 `lower_expr`
    /// 入口消费；解构目标不触发。
    pub(crate) fn stage_named_eval_for_binding(
        &mut self,
        target: &swc_ast::Pat,
        value_expr: &swc_ast::Expr,
    ) {
        if let swc_ast::Pat::Ident(binding) = target
            && Self::is_anonymous_fn_definition(value_expr)
        {
            self.named_eval_hint = Some(binding.id.sym.to_string());
        }
    }

    /// 回填已入模函数的 JS 可见 `name` / `length` 元数据。
    pub(crate) fn set_function_js_metadata(
        &mut self,
        function_id: FunctionId,
        js_name: Option<&str>,
        js_length: u32,
    ) {
        if let Some(function) = self.module.function_mut(function_id) {
            if let Some(name) = js_name {
                function.set_js_name(name);
            }
            function.set_js_length(js_length);
        }
    }

    /// 回填已入模函数的 [[SourceText]]：方法/访问器等定义点在函数入模后才知道
    /// 准确的 MethodDefinition 文本（含 `static` 剥离），此处覆盖通用路径的值。
    pub(crate) fn set_function_source_text(&mut self, function_id: FunctionId, text: Option<String>) {
        if let (Some(function), Some(text)) = (self.module.function_mut(function_id), text) {
            function.set_source_text(text);
        }
    }

    /// 发射运行时 SetFunctionName（ES §10.2.9）：`key_value` 为 ToPropertyKey
    /// 之后的键（字符串 / symbol / 数字），宿主按 symbol description 与前缀
    /// 规则合成 `name` 并写入 callable 侧表。
    pub(crate) fn emit_runtime_set_function_name(
        &mut self,
        block: BasicBlockId,
        function_value: ValueId,
        key_value: ValueId,
        prefix: AccessorPrefix,
    ) {
        let prefix_const = self
            .module
            .add_constant(Constant::Number(prefix.wire_code()));
        let prefix_value = self.alloc_value();
        self.current_function.append_instruction(
            block,
            Instruction::Const {
                dest: prefix_value,
                constant: prefix_const,
            },
        );
        self.current_function.append_instruction(
            block,
            Instruction::CallBuiltin {
                dest: None,
                builtin: Builtin::FunctionSetName,
                args: vec![function_value, key_value, prefix_value],
            },
        );
    }

    /// 方法 / 访问器的 SetFunctionName：静态键在降级期写入 js 元数据，
    /// 动态键（计算键 / 数字键）发射运行时设置。`method_value` 为已物化的
    /// 函数值，`key_value` 为属性定义使用的键值。
    pub(crate) fn apply_method_js_name(
        &mut self,
        block: BasicBlockId,
        function_id: FunctionId,
        method_value: ValueId,
        key: &swc_ast::PropName,
        key_value: ValueId,
        prefix: AccessorPrefix,
    ) {
        match Self::static_prop_js_name(key) {
            Some(name) => {
                if let Some(function) = self.module.function_mut(function_id) {
                    function.set_js_name(prefix.apply(&name));
                }
            }
            None => {
                // 动态键的函数在键求值前不可观测：先写空串占位，运行时覆盖。
                if let Some(function) = self.module.function_mut(function_id) {
                    function.set_js_name("");
                }
                self.emit_runtime_set_function_name(block, method_value, key_value, prefix);
            }
        }
    }

    /// 表达式是否为匿名函数定义（IsAnonymousFunctionDefinition，ES §8.4.5）：
    /// 无名函数表达式、箭头函数、无名类表达式；括号与 TS 类型断言透传。
    pub(crate) fn is_anonymous_fn_definition(expr: &swc_ast::Expr) -> bool {
        match expr {
            swc_ast::Expr::Fn(fn_expr) => fn_expr.ident.is_none(),
            swc_ast::Expr::Arrow(_) => true,
            swc_ast::Expr::Class(class_expr) => class_expr.ident.is_none(),
            swc_ast::Expr::Paren(paren) => Self::is_anonymous_fn_definition(&paren.expr),
            swc_ast::Expr::TsTypeAssertion(inner) => Self::is_anonymous_fn_definition(&inner.expr),
            swc_ast::Expr::TsConstAssertion(inner) => Self::is_anonymous_fn_definition(&inner.expr),
            swc_ast::Expr::TsNonNull(inner) => Self::is_anonymous_fn_definition(&inner.expr),
            swc_ast::Expr::TsAs(inner) => Self::is_anonymous_fn_definition(&inner.expr),
            swc_ast::Expr::TsSatisfies(inner) => Self::is_anonymous_fn_definition(&inner.expr),
            swc_ast::Expr::TsInstantiation(inner) => Self::is_anonymous_fn_definition(&inner.expr),
            _ => false,
        }
    }
}

/// 数字属性键的字符串形式（ToString(Number) 的属性键子集：整数直出、
/// NaN / ±Infinity 特判），与运行时数字键的字符串化保持一致。
pub(crate) fn js_number_property_key(value: f64) -> String {
    if value.is_nan() {
        return "NaN".into();
    }
    if value.is_infinite() {
        return if value.is_sign_negative() {
            "-Infinity".into()
        } else {
            "Infinity".into()
        };
    }
    if value.fract() == 0.0 && value.abs() < 1e21 {
        return format!("{}", value as i64);
    }
    format!("{value}")
}
