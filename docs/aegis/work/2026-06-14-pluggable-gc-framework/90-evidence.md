# Evidence — 可插拔 GC 框架实施

## P0: SIZE_CLASSES validation (T0.1-T0.2)
**Status**: DONE — no changes needed

Evidence:
- Size formulas verified against source: object `16 + cap*32` (compiler_helpers.rs:65-71, runtime_heap.rs:13), array `16 + cap*8` (compiler_array_helpers.rs:20-26)
- Capacity policy: `max(4, N)` for literals (lowerer_jsx_objects.rs:777,1412); host objects cap 2/3/4; growth = `cap*2`
- Dominant allocations hit class boundaries with 0 slack:
  - cap-4 object = 144B → class 144 (modal object: `{}`, descriptors, env, small literals)
  - cap-4 array = 48B → class 48 (modal array: `[]`, small literals)
  - cap-2 host object = 80B → class 80 (all-settled result)
- All small-cap object sizes (cap 0-6) hit classes 16/48/80/112/144/176/208 exactly
- No realistic size jumps 2+ classes; worst slack is prototype singletons (cap 64 → 4096 class, 2 per module, negligible)
- Conclusion: frozen table calibrated to 32B slot stride + max(4,N) policy → near-zero internal fragmentation

Command: read-only analysis (no build). Formulas verified by reading $obj_new size computation.
