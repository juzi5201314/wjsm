// sync `yield*` 收到 received return/throw 时向内层迭代器转发（§27.5.3.7
// 步骤 7.b/7.c）。.expected 与 Node v22 输出逐字节一致。
// 内层结果对象统一 {value, done} 键序：宿主把外层结果重新包装为
// {value, done}，与 Node 透传内层对象时的键序在 JSON 渲染上保持一致。

function makeInner(log) {
  return {
    [Symbol.iterator]() { return this; },
    next() { return { value: 'n', done: false }; },
    return(v) { log('inner return', v); return { value: 'r-' + v, done: true }; },
    throw(v) { log('inner throw', v); return { value: 't-' + v, done: false }; },
  };
}
const log = (...args) => console.log(...args);

// 1. return 转发：inner.return 被调用，done 结果作为外层 return 值（步骤 7.c.viii）
{
  function* outer() { yield* makeInner(log); }
  const it = outer();
  console.log('1a', JSON.stringify(it.next()));
  console.log('1b', JSON.stringify(it.return(9)));
}

// 2. return 转发 done:false：继续委托（步骤 7.c.x）
{
  function* outer() { yield* { [Symbol.iterator]() { return this; }, next() { return { value: 'n', done: false }; }, return(v) { return { value: 'still-' + v, done: false }; } }; }
  const it = outer();
  console.log('2a', JSON.stringify(it.next()));
  console.log('2b', JSON.stringify(it.return(9)));
  console.log('2c', JSON.stringify(it.next()));
  console.log('2d', JSON.stringify(it.return(7)));
}

// 3. return 方法缺失：外层直接 ReturnCompletion(received)（步骤 7.c.iii）
{
  function* outer() { yield* { [Symbol.iterator]() { return this; }, next() { return { value: 'n', done: false }; } }; console.log('unreachable'); }
  const it = outer();
  console.log('3a', JSON.stringify(it.next()));
  console.log('3b', JSON.stringify(it.return(9)));
}

// 4. throw 转发 done:false：yield 转发结果并继续委托（步骤 7.b.ii.7）
{
  function* outer() { yield* makeInner(log); }
  const it = outer();
  console.log('4a', JSON.stringify(it.next()));
  console.log('4b', JSON.stringify(it.throw('boom')));
  console.log('4c', JSON.stringify(it.next()));
}

// 5. throw 转发 done:true：yield* 正常完成，外层 body 继续（步骤 7.b.ii.6）
{
  function* outer() {
    const r = yield* { [Symbol.iterator]() { return this; }, next() { return { value: 'n', done: false }; }, throw(v) { return { value: 'tv-' + v, done: true }; } };
    console.log('after yield*', r);
    yield 'tail';
  }
  const it = outer();
  it.next();
  console.log('5a', JSON.stringify(it.throw('z')));
  console.log('5b', JSON.stringify(it.next()));
}

// 6. throw 缺失、有 return：先按 normal completion 关闭（IteratorClose 调用
//    return，不传实参）再抛 TypeError（步骤 7.b.iii）
{
  function* outer() { yield* { [Symbol.iterator]() { return this; }, next() { return { value: 'n', done: false }; }, return(v) { console.log('close return', v); return { value: undefined, done: true }; } }; }
  const it = outer();
  console.log('6a', JSON.stringify(it.next()));
  try { it.throw('boom'); } catch (e) { console.log('6b', e.constructor.name, e.message); }
}

// 7. throw 与 return 皆缺失：close 为空操作，直接 TypeError
{
  function* outer() { yield* { [Symbol.iterator]() { return this; }, next() { return { value: 'n', done: false }; } }; }
  const it = outer();
  it.next();
  try { it.throw('boom'); } catch (e) { console.log('7a', e.constructor.name, e.message); }
}

// 8. 非对象结果：throw/return 转发结果必须是对象（步骤 7.b.ii.4 / 7.c.vi）
{
  function* g1() { yield* { [Symbol.iterator]() { return this; }, next() { return { value: 1, done: false }; }, throw() { return 42; } }; }
  const a = g1();
  a.next();
  try { a.throw('x'); } catch (e) { console.log('8a', e.constructor.name, e.message); }
  function* g2() { yield* { [Symbol.iterator]() { return this; }, next() { return { value: 1, done: false }; }, return() { return 'nope'; } }; }
  const b = g2();
  b.next();
  try { b.return(9); } catch (e) { console.log('8b', e.constructor.name, e.message); }
}

// 9. return 转发先于外层 finally：转发发生在 yield* 站点，ReturnCompletion
//    再穿越外层 finalizer
{
  function* outer() {
    try {
      yield* { [Symbol.iterator]() { return this; }, next() { return { value: 'n', done: false }; }, return(v) { console.log('inner return', v); return { value: 'rv-' + v, done: true }; } };
    } finally { console.log('outer finally'); }
  }
  const it = outer();
  it.next();
  console.log('9a', JSON.stringify(it.return(9)));
}

// 10. GetMethod 的 getter 抛出：异常在 yield* 站点传播（步骤 7.b.i `? GetMethod`）
{
  function* outer() { yield* { [Symbol.iterator]() { return this; }, next() { return { value: 'n', done: false }; }, get throw() { throw new Error('getter-boom'); } }; }
  const it = outer();
  it.next();
  try { it.throw('x'); } catch (e) { console.log('10a', e.message); }
}

// 11. 非可调用 throw/return：GetMethod 抛 TypeError（§7.3.10）
{
  function* g1() { yield* { [Symbol.iterator]() { return this; }, next() { return { value: 1, done: false }; }, throw: 42 }; }
  const a = g1();
  a.next();
  try { a.throw('x'); } catch (e) { console.log('11a', e.constructor.name, e.message); }
  function* g2() { yield* { [Symbol.iterator]() { return this; }, next() { return { value: 1, done: false }; }, return: 'x' }; }
  const b = g2();
  b.next();
  try { b.return(9); } catch (e) { console.log('11b', e.constructor.name, e.message); }
}

// 12. 内层为真实生成器：throw 注入被内层 catch 接住并续 yield，return 注入
//     跑内层 finally 后完成
{
  function* inner() {
    try { yield 'a'; yield 'b'; }
    catch (e) { console.log('inner caught', e); yield 'c'; }
    finally { console.log('inner finally'); }
    return 'inner-done';
  }
  function* outer() { const r = yield* inner(); console.log('outer got', r); yield 'tail'; }
  let it = outer();
  console.log('12a', JSON.stringify(it.next()));
  console.log('12b', JSON.stringify(it.throw('boom')));
  console.log('12c', JSON.stringify(it.next()));
  console.log('12d', JSON.stringify(it.next()));
  it = outer();
  it.next();
  console.log('12e', JSON.stringify(it.return(9)));
  console.log('12f', JSON.stringify(it.next()));
}

// 13. throw 转发后再 return 转发：转发彼此独立，条目状态正确推进
{
  function* outer() { yield* makeInner(log); }
  const it = outer();
  it.next();
  console.log('13a', JSON.stringify(it.throw('t1')));
  console.log('13b', JSON.stringify(it.return('r1')));
}
