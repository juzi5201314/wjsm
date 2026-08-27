// eval 完成值语义：ECMAScript 完成值经 UpdateEmpty 归一后与 Node 一致。
function show(label, value) {
  console.log(label, JSON.stringify(value));
}

// ── 表达式语句 / 声明（empty 完成值透传）──────────────────────────────
show("expr:", eval("1; 2"));
show("empty:", eval(""));
show("decl-keeps:", eval("42; var x = 1"));
show("fn-decl-keeps:", eval("7; function f() { 42; }"));
show("block-empty-keeps:", eval("42; { }"));

// ── if：UpdateEmpty(branch, undefined) ────────────────────────────────
show("if-false:", eval("42; if (false) 1;"));
show("if-true-empty:", eval("42; if (true) {}"));
show("if-then:", eval("1; if (true) { 2; } else { 3; }"));
show("if-else:", eval("1; if (false) { 2; } else { 3; }"));

// ── try/catch/finally ────────────────────────────────────────────────
show("try-normal:", eval("try { 1 } catch (e) { 2 }"));
show("try-caught:", eval("try { throw 0 } catch (e) { 2 }"));
show("try-partial-discarded:", eval("try { 1; throw 0 } catch (e) {}"));
show("try-empty:", eval("2; try { } catch (e) {}"));
show("finally-discarded:", eval("try { 1 } finally { 2 }"));
show("finally-empty-body:", eval("2; try { } finally { 3 }"));
show("catch-then-finally:", eval("1; try { 2; throw 0 } catch (e) { } finally { 9 }"));
show("catch-value-finally:", eval("try { throw 8 } catch (e) { e } finally { 3 }"));
show("nested-finally-caught:", eval("try { try { 1; throw 0 } finally { 2 } } catch (e) { 5 }"));

// ── 循环：V = undefined 起步，非 empty 迭代覆盖 ───────────────────────
show("while-skipped:", eval("1; while (false) { 2 }"));
show("while-last:", eval("var c = 3; while (c--) { 5; }"));
show("do-while:", eval("do { 1 } while (false)"));
show("for:", eval("for (var i = 0; i < 3; i++) { i * 10 }"));
show("for-of:", eval("for (const v of [1, 2, 3]) { v }"));
show("for-in:", eval("for (const k in { a: 1 }) { k }"));
show("loop-break:", eval("while (true) { break }"));
show("loop-labeled-break:", eval("1; c: while (true) { 2; break c }"));
show("loop-continue:", eval("var i = 0; while (i < 3) { i++; if (i === 2) continue; i * 100 }"));

// ── switch：V = undefined 起步 + fall-through ─────────────────────────
show("switch-match:", eval("switch (1) { case 1: 10; break; default: 20 }"));
show("switch-fallthrough:", eval("switch (2) { case 1: 10; case 2: 20; case 3: 30 }"));
show("switch-nomatch:", eval("switch (9) { case 1: 10; break; }"));

// ── 标签块 break：break 携带当前完成值 ───────────────────────────────
show("labeled-break:", eval("l: { 1; break l; 2 }"));
show("labeled-empty:", eval("9; l3: {}"));

// ── finally 的 abrupt 完成：finally 内部值线程化生效 ─────────────────
show("finally-break-normal:", eval("l: { 1; try { 2 } finally { break l; } 4; }"));
show("finally-break-throw:", eval("l: { 1; try { throw 0 } finally { break l; } 2; }"));
show("finally-break-value:", eval("l: { 1; try { 2 } finally { 3; break l; } 4; }"));
show("finally-crossed-break:", eval("lw: while (true) { 5; try { 6; break lw } finally { 7 } }"));
show(
  "finally-crossed-continue:",
  eval("var n = 0; lw2: while (n < 2) { n++; try { 8; continue lw2 } finally { 9 } }")
);

// ── 嵌套函数体不参与完成值；顶层调用值参与 ───────────────────────────
show("call-value:", eval("function g() { return 9 } g()"));
show("iife-undefined:", eval("5; (function () { 42; })()"));
show("nested-if-fn:", eval("5; function g4() { if (1) { 42; } }"));

// ── 顶层调用语句抛异常必须传播，不得吞掉 ─────────────────────────────
try {
  eval("function boom() { throw 7 } boom(); 2");
  console.log("throw-propagates: MISSED");
} catch (e) {
  show("throw-propagates:", e);
}

// ── 标签块 break 之后的语句仍可达（exit 可达性）──────────────────────
show("labeled-after:", eval("l: { break l } 'after'"));
