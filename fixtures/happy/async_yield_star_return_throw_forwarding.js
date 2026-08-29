// async `yield*` 收到 received return/throw 时向内层迭代器转发（§27.5.3.7
// 步骤 7.b/7.c 的 async 形态）。.expected 与 Node v22 输出逐字节一致。
// return 恢复值先经 AsyncGeneratorUnwrapYieldResumption 的 Await 解包，
// 转发结果经 Await 后才做对象校验与 done 分支。

function makeInner(log) {
  return {
    [Symbol.asyncIterator]() { return this; },
    next() { return Promise.resolve({ value: 'n', done: false }); },
    return(v) { log('inner return', v); return Promise.resolve({ value: 'r-' + v, done: true }); },
    throw(v) { log('inner throw', v); return Promise.resolve({ value: 't-' + v, done: false }); },
  };
}
const log = (...args) => console.log(...args);

async function main() {
  // 1. return 转发：结果 promise 被 Await，done 后 ReturnCompletion（步骤 7.c.v/viii）
  {
    async function* outer() { yield* makeInner(log); }
    const it = outer();
    console.log('1a', JSON.stringify(await it.next()));
    console.log('1b', JSON.stringify(await it.return(9)));
  }

  // 2. return 转发 done:false：继续委托（步骤 7.c.ix）
  {
    async function* outer() { yield* { [Symbol.asyncIterator]() { return this; }, next() { return { value: 'n', done: false }; }, return(v) { return Promise.resolve({ value: 'still-' + v, done: false }); } }; }
    const it = outer();
    console.log('2a', JSON.stringify(await it.next()));
    console.log('2b', JSON.stringify(await it.return(9)));
    console.log('2c', JSON.stringify(await it.next()));
  }

  // 3. return 方法缺失：received 被 Await 后 ReturnCompletion（步骤 7.c.iii）
  {
    async function* outer() { yield* { [Symbol.asyncIterator]() { return this; }, next() { return { value: 'n', done: false }; } }; console.log('unreachable'); }
    const it = outer();
    console.log('3a', JSON.stringify(await it.next()));
    console.log('3b', JSON.stringify(await it.return(Promise.resolve(77))));
  }

  // 4. return 恢复值的 unwrap Await：inner.return 收到解包后的值
  {
    async function* outer() { yield* { [Symbol.asyncIterator]() { return this; }, next() { return { value: 'n', done: false }; }, return(v) { console.log('inner return got', typeof v, v); return { value: 'rv', done: true }; } }; }
    const it = outer();
    await it.next();
    console.log('4a', JSON.stringify(await it.return(Promise.resolve(55))));
  }

  // 5. throw 转发 done:false：继续委托（步骤 7.b.ii.7）
  {
    async function* outer() { yield* makeInner(log); }
    const it = outer();
    console.log('5a', JSON.stringify(await it.next()));
    console.log('5b', JSON.stringify(await it.throw('boom')));
    console.log('5c', JSON.stringify(await it.next()));
  }

  // 6. throw 转发 done:true：yield* 正常完成，外层 body 继续（步骤 7.b.ii.6）
  {
    async function* outer() {
      const r = yield* { [Symbol.asyncIterator]() { return this; }, next() { return { value: 'n', done: false }; }, throw(v) { return { value: 'tv-' + v, done: true }; } };
      console.log('after yield*', r);
      yield 'tail';
    }
    const it = outer();
    await it.next();
    console.log('6a', JSON.stringify(await it.throw('z')));
    console.log('6b', JSON.stringify(await it.next()));
  }

  // 7. throw 缺失、有 return：AsyncIteratorClose（return 不传实参、结果被
  //    Await）后抛 TypeError（步骤 7.b.iii）
  {
    async function* outer() { yield* { [Symbol.asyncIterator]() { return this; }, next() { return { value: 'n', done: false }; }, return(v) { console.log('close return', v); return Promise.resolve({ value: undefined, done: true }); } }; }
    const it = outer();
    console.log('7a', JSON.stringify(await it.next()));
    try { await it.throw('boom'); } catch (e) { console.log('7b', e.constructor.name, e.message); }
  }

  // 8. throw 缺失、close 结果拒绝：rejection 胜出（§7.4.10 步骤 5）
  {
    async function* outer() { yield* { [Symbol.asyncIterator]() { return this; }, next() { return { value: 'n', done: false }; }, return() { return Promise.reject(new Error('close-rej')); } }; }
    const it = outer();
    await it.next();
    try { await it.throw('x'); } catch (e) { console.log('8a', e.message); }
  }

  // 9. inner.throw 结果拒绝：rejection 传播（步骤 7.b.ii.2 `? Await`）
  {
    async function* outer() { yield* { [Symbol.asyncIterator]() { return this; }, next() { return { value: 'n', done: false }; }, throw() { return Promise.reject(new Error('throw-rej')); } }; }
    const it = outer();
    await it.next();
    try { await it.throw('x'); } catch (e) { console.log('9a', e.message); }
  }

  // 10. 非对象结果（Await 后校验，步骤 7.b.ii.4 / 7.c.vi）
  {
    async function* g1() { yield* { [Symbol.asyncIterator]() { return this; }, next() { return { value: 'n', done: false }; }, throw() { return Promise.resolve(42); } }; }
    const a = g1();
    await a.next();
    try { await a.throw('x'); } catch (e) { console.log('10a', e.constructor.name, e.message); }
    async function* g2() { yield* { [Symbol.asyncIterator]() { return this; }, next() { return { value: 'n', done: false }; }, return() { return 'nope'; } }; }
    const b = g2();
    await b.next();
    try { await b.return(9); } catch (e) { console.log('10b', e.constructor.name, e.message); }
  }

  // 11. throw 与 return 皆缺失：直接 TypeError
  {
    async function* outer() { yield* { [Symbol.asyncIterator]() { return this; }, next() { return { value: 'n', done: false }; } }; }
    const it = outer();
    await it.next();
    try { await it.throw('boom'); } catch (e) { console.log('11a', e.constructor.name, e.message); }
  }

  // 12. 内层为 async generator：throw 注入被内层 catch 接住，return 注入跑
  //     内层 finally 后完成
  {
    async function* inner() {
      try { yield 'a'; yield 'b'; }
      catch (e) { console.log('inner caught', e); yield 'c'; }
      finally { console.log('inner finally'); }
      return 'inner-done';
    }
    async function* outer() { const r = yield* inner(); console.log('outer got', r); yield 'tail'; }
    let it = outer();
    console.log('12a', JSON.stringify(await it.next()));
    console.log('12b', JSON.stringify(await it.throw('boom')));
    console.log('12c', JSON.stringify(await it.next()));
    console.log('12d', JSON.stringify(await it.next()));
    it = outer();
    await it.next();
    console.log('12e', JSON.stringify(await it.return(9)));
    console.log('12f', JSON.stringify(await it.next()));
  }

  // 13. 内层为 sync 可迭代（AsyncFromSync 包装，§27.1.6.2.3）：sync throw
  //     被调用，done 结果作为 yield* 的 normal 完成值
  {
    function makeSync() { return { [Symbol.iterator]() { return this; }, next() { return { value: 's', done: false }; }, throw(v) { console.log('sync throw', v); return { value: 'st-' + v, done: true }; } }; }
    async function* outer() { const r = yield* makeSync(); console.log('after', r); yield 'tail'; }
    const it = outer();
    await it.next();
    console.log('13a', JSON.stringify(await it.throw('z')));
  }

  // 14. 内层为 sync 可迭代的 return 转发：unwrap Await 后转发给 sync return
  {
    function makeSync() { return { [Symbol.iterator]() { return this; }, next() { return { value: 's', done: false }; }, return(v) { console.log('sync return', v); return { value: 'sr-' + v, done: true }; } }; }
    async function* outer() { yield* makeSync(); }
    const it = outer();
    await it.next();
    console.log('14a', JSON.stringify(await it.return(9)));
  }
}
main();
