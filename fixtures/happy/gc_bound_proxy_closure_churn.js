// BoundFunction / Proxy / Closure 多轮 GC 存活回归。
// 目标：侧表记录的 bound this/args、proxy target/handler、closure env 在多轮回收后仍可访问。

function makeClosure(seed) {
  let env = { base: seed, hits: 0, nested: { add: 5 } };
  return function(delta) {
    env.hits = env.hits + 1;
    return env.base + env.nested.add + delta + env.hits;
  };
}

function timerCallback(argBox, delta) {
  console.log(this.prefix + argBox.extra + delta);
}

function makeBoundTimerCallback() {
  let receiver = { prefix: 100 };
  let argBox = { extra: 7 };
  return timerCallback.bind(receiver, argBox, 2);
}

function makeProxy() {
  let target = { value: 3 };
  let state = { offset: 11 };
  let handler = {
    get(t, key) {
      if (key === "combined") {
        return t.value + state.offset;
      }
      return t[key];
    },
    set(t, key, value) {
      t[key] = value + state.offset;
      return true;
    }
  };
  return new Proxy(target, handler);
}

const closure = makeClosure(30);
const boundTimerCallback = makeBoundTimerCallback();
const proxy = makeProxy();

for (let round = 0; round < 6; round++) {
  for (let i = 0; i < 900; i++) {
    const tmp = { round, i, nested: { keep: i + round }, arr: [round, i, i + 1] };
    if (tmp.nested.keep === -1) {
      console.log("unreachable");
    }
  }
  gc();
}

setTimeout(boundTimerCallback, 0);
console.log(closure(3));
console.log(proxy.combined);
proxy.value = 20;
gc();
console.log(proxy.combined);
console.log(closure(0));
