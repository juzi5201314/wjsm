// ExecContext 方法片段：global
macro_rules! exec_ctx_global {
    () => {
    fn js_global_get(&mut self) -> Value {
        self.caller
            .data()
            .js_global_object
            .load(std::sync::atomic::Ordering::Relaxed)
    }
    fn js_global_set(&mut self, obj: Value) {
        self.caller
            .data()
            .js_global_object
            .store(obj, std::sync::atomic::Ordering::Relaxed);
    }
    fn install_process_global(&mut self, global: Value) {
        let _ = crate::install_process_global_from_caller(self.caller, global);
    }
    fn install_node_web_globals(&mut self, global: Value) {
        let _ =
            crate::runtime_node_globals::install_node_web_globals_from_caller(self.caller, global);
    }
    fn value_to_number(&mut self, val: Value) -> Value {
        wjsm_builtins::core::to_number(self, val)
    }
    fn to_primitive(&mut self, val: Value, hint_number: bool) -> Value {
        let hint = if hint_number {
            crate::runtime_values::ToPrimitiveHint::Number
        } else {
            crate::runtime_values::ToPrimitiveHint::Default
        };
        crate::runtime_values::to_primitive_with_hint(self.caller, val, hint)
    }
    fn to_number(&mut self, val: Value) -> Value {
        wjsm_builtins::core::to_number(self, val)
    }
    fn to_primitive_hinted(&mut self, val: Value, hint: wjsm_host::ToPrimitiveHintKind) -> Value {
        let internal = match hint {
            wjsm_host::ToPrimitiveHintKind::Default => {
                crate::runtime_values::ToPrimitiveHint::Default
            }
            wjsm_host::ToPrimitiveHintKind::Number => {
                crate::runtime_values::ToPrimitiveHint::Number
            }
            wjsm_host::ToPrimitiveHintKind::String => {
                crate::runtime_values::ToPrimitiveHint::String
            }
        };
        crate::runtime_values::to_primitive_with_hint(self.caller, val, internal)
    }
    fn to_boolean(&mut self, val: Value) -> bool {
        crate::runtime_values::to_boolean(self.caller, val)
    }
    fn create_collection_method(&mut self, kind: &str) -> Value {
        use crate::types::{
            MapSetMethodKind, NativeCallable, WeakMapMethodKind, WeakSetMethodKind,
        };
        let callable = match kind {
            "map_set" => NativeCallable::MapSetMethod {
                kind: MapSetMethodKind::MapSet,
            },
            "map_get" => NativeCallable::MapSetMethod {
                kind: MapSetMethodKind::MapGet,
            },
            "map_has" | "set_has" => NativeCallable::MapSetMethod {
                kind: MapSetMethodKind::Has,
            },
            "map_delete" | "set_delete" => NativeCallable::MapSetMethod {
                kind: MapSetMethodKind::Delete,
            },
            "map_clear" | "set_clear" => NativeCallable::MapSetMethod {
                kind: MapSetMethodKind::Clear,
            },
            "map_size" | "set_size" => NativeCallable::MapSetMethod {
                kind: MapSetMethodKind::Size,
            },
            "map_for_each" | "set_for_each" => NativeCallable::MapSetMethod {
                kind: MapSetMethodKind::ForEach,
            },
            "map_keys" | "set_keys" => NativeCallable::MapSetMethod {
                kind: MapSetMethodKind::Keys,
            },
            "map_values" | "set_values" => NativeCallable::MapSetMethod {
                kind: MapSetMethodKind::Values,
            },
            "map_entries" | "set_entries" => NativeCallable::MapSetMethod {
                kind: MapSetMethodKind::Entries,
            },
            "set_add" => NativeCallable::MapSetMethod {
                kind: MapSetMethodKind::SetAdd,
            },
            "weakmap_set" => NativeCallable::WeakMapMethod {
                kind: WeakMapMethodKind::Set,
            },
            "weakmap_get" => NativeCallable::WeakMapMethod {
                kind: WeakMapMethodKind::Get,
            },
            "weakmap_has" => NativeCallable::WeakMapMethod {
                kind: WeakMapMethodKind::Has,
            },
            "weakmap_delete" => NativeCallable::WeakMapMethod {
                kind: WeakMapMethodKind::Delete,
            },
            "weakset_add" => NativeCallable::WeakSetMethod {
                kind: WeakSetMethodKind::Add,
            },
            "weakset_has" => NativeCallable::WeakSetMethod {
                kind: WeakSetMethodKind::Has,
            },
            "weakset_delete" => NativeCallable::WeakSetMethod {
                kind: WeakSetMethodKind::Delete,
            },
            other => {
                if let Some(v) = self.create_global_builtin(other) {
                    return v;
                }
                return value::encode_undefined();
            }
        };
        crate::create_native_callable(self.caller.data(), callable)
    }
    };
}
