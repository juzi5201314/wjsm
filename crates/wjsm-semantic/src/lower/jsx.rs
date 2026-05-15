use swc_core::common::DUMMY_SP;
use swc_core::common::Span;
use swc_core::common::Spanned;
use swc_core::ecma::ast as swc_ast;
use wjsm_ir::{
    BasicBlock, BasicBlockId, BinaryOp, Builtin, CompareOp, Constant, ConstantId, Function,
    FunctionId, Instruction, Module, PhiSource, Program, SwitchCaseTarget, Terminator, UnaryOp,
    ValueId,
};
use crate::scope_tree::{ScopeKind, VarKind, LexicalMode, ScopeTree};
use crate::cfg_builder::{FunctionBuilder, LabelContext, LabelKind, FinallyContext, TryContext, StmtFlow};
use crate::builtins::*;
use crate::eval_helpers::*;
use crate::kind_strings::*;
use crate::{LoweringError, Diagnostic};
use super::lowerer::{Lowerer, ActiveUsingVar, AsyncContextState, HoistedVar, CapturedBinding, EVAL_SCOPE_ENV_PARAM, WK_SYMBOL_ITERATOR, WK_SYMBOL_SPECIES, WK_SYMBOL_TO_STRING_TAG, WK_SYMBOL_ASYNC_ITERATOR, WK_SYMBOL_HAS_INSTANCE, WK_SYMBOL_TO_PRIMITIVE, WK_SYMBOL_DISPOSE, WK_SYMBOL_MATCH, WK_SYMBOL_ASYNC_DISPOSE};

impl Lowerer {
    pub(crate) fn lower_jsx_element(
        &mut self,
        jsx_el: &swc_ast::JSXElement,
        block: BasicBlockId,
    ) -> Result<ValueId, LoweringError> {
        // 降低 tag 名
        let tag_val = self.lower_jsx_element_name(&jsx_el.opening.name, block)?;

        // 降低 props
        let props_val = self.lower_jsx_attrs(&jsx_el.opening.attrs, block)?;

        // 降低 children（作为数组）
        let children_val = self.lower_jsx_children(&jsx_el.children, block)?;

        // 调用 jsx_create_element(tag, props, children)
        let dest = self.alloc_value();
        self.current_function.append_instruction(
            block,
            Instruction::CallBuiltin {
                dest: Some(dest),
                builtin: Builtin::JsxCreateElement,
                args: vec![tag_val, props_val, children_val],
            },
        );
        Ok(dest)
    }

    pub(crate) fn lower_jsx_fragment(
        &mut self,
        jsx_frag: &swc_ast::JSXFragment,
        block: BasicBlockId,
    ) -> Result<ValueId, LoweringError> {
        // Fragment 使用字符串标记 "$JsxFragment"
        let tag_str = "$JsxFragment".to_string();
        let tag_const = self.module.add_constant(Constant::String(tag_str));
        let tag_val = self.alloc_value();
        self.current_function.append_instruction(
            block,
            Instruction::Const {
                dest: tag_val,
                constant: tag_const,
            },
        );

        // Fragment 的 props 为 null
        let null_const = self.module.add_constant(Constant::Null);
        let props_val = self.alloc_value();
        self.current_function.append_instruction(
            block,
            Instruction::Const {
                dest: props_val,
                constant: null_const,
            },
        );

        // 收集 children
        let children_val = self.lower_jsx_children(&jsx_frag.children, block)?;

        // 调用 jsx_create_element(tag, null, children)
        let dest = self.alloc_value();
        self.current_function.append_instruction(
            block,
            Instruction::CallBuiltin {
                dest: Some(dest),
                builtin: Builtin::JsxCreateElement,
                args: vec![tag_val, props_val, children_val],
            },
        );
        Ok(dest)
    }

    pub(crate) fn lower_jsx_element_name(
        &mut self,
        name: &swc_ast::JSXElementName,
        block: BasicBlockId,
    ) -> Result<ValueId, LoweringError> {
        match name {
            swc_ast::JSXElementName::Ident(ident) => {
                // HTML 标签名 → 字符串常量
                let tag_str = ident.sym.to_string();
                let tag_const = self.module.add_constant(Constant::String(tag_str));
                let tag_val = self.alloc_value();
                self.current_function.append_instruction(
                    block,
                    Instruction::Const {
                        dest: tag_val,
                        constant: tag_const,
                    },
                );
                Ok(tag_val)
            }
            swc_ast::JSXElementName::JSXMemberExpr(member_expr) => {
                // <Foo.Bar /> → 降低为成员表达式
                let obj_val = self.lower_jsx_object(&member_expr.obj, block)?;
                let prop_name = member_expr.prop.sym.to_string();
                let prop_const = self.module.add_constant(Constant::String(prop_name));
                let prop_key = self.alloc_value();
                self.current_function.append_instruction(
                    block,
                    Instruction::Const {
                        dest: prop_key,
                        constant: prop_const,
                    },
                );
                let dest = self.alloc_value();
                self.current_function.append_instruction(
                    block,
                    Instruction::GetProp {
                        dest,
                        object: obj_val,
                        key: prop_key,
                    },
                );
                Ok(dest)
            }
            swc_ast::JSXElementName::JSXNamespacedName(ns_name) => {
                // <ns:tag /> → 字符串 "ns:tag"
                let tag_str = format!("{}:{}", ns_name.ns.sym, ns_name.name.sym);
                let tag_const = self.module.add_constant(Constant::String(tag_str));
                let tag_val = self.alloc_value();
                self.current_function.append_instruction(
                    block,
                    Instruction::Const {
                        dest: tag_val,
                        constant: tag_const,
                    },
                );
                Ok(tag_val)
            }
        }
    }

    pub(crate) fn lower_jsx_object(
        &mut self,
        obj: &swc_ast::JSXObject,
        block: BasicBlockId,
    ) -> Result<ValueId, LoweringError> {
        match obj {
            swc_ast::JSXObject::JSXMemberExpr(member_expr) => {
                let obj_val = self.lower_jsx_object(&member_expr.obj, block)?;
                let prop_name = member_expr.prop.sym.to_string();
                let prop_const = self.module.add_constant(Constant::String(prop_name));
                let prop_key = self.alloc_value();
                self.current_function.append_instruction(
                    block,
                    Instruction::Const {
                        dest: prop_key,
                        constant: prop_const,
                    },
                );
                let dest = self.alloc_value();
                self.current_function.append_instruction(
                    block,
                    Instruction::GetProp {
                        dest,
                        object: obj_val,
                        key: prop_key,
                    },
                );
                Ok(dest)
            }
            swc_ast::JSXObject::Ident(ident) => {
                self.lower_ident(ident, block)
            }
        }
    }

    pub(crate) fn lower_jsx_attrs(
        &mut self,
        attrs: &[swc_ast::JSXAttrOrSpread],
        block: BasicBlockId,
    ) -> Result<ValueId, LoweringError> {
        if attrs.is_empty() {
            // 无属性 → null
            let null_const = self.module.add_constant(Constant::Null);
            let null_val = self.alloc_value();
            self.current_function.append_instruction(
                block,
                Instruction::Const {
                    dest: null_val,
                    constant: null_const,
                },
            );
            return Ok(null_val);
        }

        // 创建 props 对象
        let capacity = std::cmp::max(4, attrs.len() as u32);
        let obj_dest = self.alloc_value();
        self.current_function.append_instruction(
            block,
            Instruction::NewObject {
                dest: obj_dest,
                capacity,
            },
        );

        for attr_or_spread in attrs {
            match attr_or_spread {
                swc_ast::JSXAttrOrSpread::JSXAttr(attr) => {
                    let attr_name = match &attr.name {
                        swc_ast::JSXAttrName::Ident(ident) => ident.sym.to_string(),
                        swc_ast::JSXAttrName::JSXNamespacedName(ns_name) => {
                            format!("{}:{}", ns_name.ns.sym, ns_name.name.sym)
                        }
                    };

                    let attr_value = if let Some(ref value) = attr.value {
                        match &*value {
                            swc_ast::JSXAttrValue::Str(s) => {
                                let str_val = s.value.to_string_lossy().into_owned();
                                let const_id = self.module.add_constant(Constant::String(str_val));
                                let val = self.alloc_value();
                                self.current_function.append_instruction(
                                    block,
                                    Instruction::Const {
                                        dest: val,
                                        constant: const_id,
                                    },
                                );
                                val
                            }
                            swc_ast::JSXAttrValue::JSXExprContainer(expr_container) => {
                                match &expr_container.expr {
                                    swc_ast::JSXExpr::Expr(expr) => {
                                        self.lower_expr(expr, block)?
                                    }
                                    swc_ast::JSXExpr::JSXEmptyExpr(_) => {
                                        // 空表达式 → true
                                        let true_const =
                                            self.module.add_constant(Constant::Bool(true));
                                        let val = self.alloc_value();
                                        self.current_function.append_instruction(
                                            block,
                                            Instruction::Const {
                                                dest: val,
                                                constant: true_const,
                                            },
                                        );
                                        val
                                    }
                                }
                            }
                            swc_ast::JSXAttrValue::JSXElement(el) => {
                                self.lower_jsx_element(&el, block)?
                            }
                            swc_ast::JSXAttrValue::JSXFragment(frag) => {
                                self.lower_jsx_fragment(&frag, block)?
                            }
                        }
                    } else {
                        // 无值属性（如 <input disabled />）→ true
                        let true_const = self.module.add_constant(Constant::Bool(true));
                        let val = self.alloc_value();
                        self.current_function.append_instruction(
                            block,
                            Instruction::Const {
                                dest: val,
                                constant: true_const,
                            },
                        );
                        val
                    };

                    // SetProp(obj, attr_name, attr_value)
                    let key_const = self.module.add_constant(Constant::String(attr_name));
                    let key_dest = self.alloc_value();
                    self.current_function.append_instruction(
                        block,
                        Instruction::Const {
                            dest: key_dest,
                            constant: key_const,
                        },
                    );
                    self.current_function.append_instruction(
                        block,
                        Instruction::SetProp {
                            object: obj_dest,
                            key: key_dest,
                            value: attr_value,
                        },
                    );
                }
                swc_ast::JSXAttrOrSpread::SpreadElement(spread) => {
                    let source = self.lower_expr(&spread.expr, block)?;
                    self.current_function.append_instruction(
                        block,
                        Instruction::ObjectSpread {
                            dest: obj_dest,
                            source,
                        },
                    );
                }
            }
        }

        Ok(obj_dest)
    }

    pub(crate) fn lower_jsx_children(
        &mut self,
        children: &[swc_ast::JSXElementChild],
        block: BasicBlockId,
    ) -> Result<ValueId, LoweringError> {
        if children.is_empty() {
            // 无 children → null
            let null_const = self.module.add_constant(Constant::Null);
            let null_val = self.alloc_value();
            self.current_function.append_instruction(
                block,
                Instruction::Const {
                    dest: null_val,
                    constant: null_const,
                },
            );
            return Ok(null_val);
        }

        // 创建 children 数组
        let arr = self.alloc_value();
        self.current_function.append_instruction(
            block,
            Instruction::NewArray {
                dest: arr,
                capacity: children.len() as u32,
            },
        );

        for child in children {
            let child_val = match child {
                swc_ast::JSXElementChild::JSXText(text) => {
                    let trimmed = text.value.trim();
                    if trimmed.is_empty() {
                        continue;
                    }
                    let str_const = self.module.add_constant(Constant::String(trimmed.to_string()));
                    let val = self.alloc_value();
                    self.current_function.append_instruction(
                        block,
                        Instruction::Const {
                            dest: val,
                            constant: str_const,
                        },
                    );
                    val
                }
                swc_ast::JSXElementChild::JSXExprContainer(expr_container) => {
                    match &expr_container.expr {
                        swc_ast::JSXExpr::Expr(expr) => self.lower_expr(expr, block)?,
                        swc_ast::JSXExpr::JSXEmptyExpr(_) => continue,
                    }
                }
                swc_ast::JSXElementChild::JSXElement(el) => {
                    self.lower_jsx_element(el, block)?
                }
                swc_ast::JSXElementChild::JSXFragment(frag) => {
                    self.lower_jsx_fragment(frag, block)?
                }
                _ => continue,
            };
            self.current_function.append_instruction(
                block,
                Instruction::CallBuiltin {
                    dest: None,
                    builtin: Builtin::ArrayPush,
                    args: vec![arr, child_val],
                },
            );
        }

        Ok(arr)
    }

}
