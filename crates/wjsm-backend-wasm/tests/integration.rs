#[path = "integration/analysis_liveness.rs"]
mod analysis_liveness;
#[path = "integration/async_switch_compile.rs"]
mod async_switch_compile;
#[path = "integration/compiler_gc_analysis_spill.rs"]
mod compiler_gc_analysis_spill;
#[path = "integration/gc_alloc_window.rs"]
mod gc_alloc_window;
#[path = "integration/host_import_registry.rs"]
mod host_import_registry;
#[path = "integration/primordial_strings.rs"]
mod primordial_strings;
#[path = "integration/shadow_stack_layout.rs"]
mod shadow_stack_layout;
#[path = "integration/startup_bootstrap_exports.rs"]
mod startup_bootstrap_exports;
#[path = "integration/var_slot_liveness_gc_long_loop.rs"]
mod var_slot_liveness_gc_long_loop;

#[path = "integration/heap_memory64.rs"]
mod heap_memory64;
