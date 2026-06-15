// GC safepoint spill stress: many live handle locals across an allocation safepoint.
// Forces the prologue/epilogue to spill multiple handles at once.
function gc_spill_stress() {
    const a = { x: 1 };
    const b = { y: 2 };
    const c = { z: 3 };
    const d = { w: 4 };
    const e = { v: 5 };
    // Allocation here while a,b,c,d,e all live -> spills 5 handles.
    const f = { u: 6 };
    return [a, b, c, d, e, f];
}
gc_spill_stress();
