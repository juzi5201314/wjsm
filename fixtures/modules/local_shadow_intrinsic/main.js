function setTimeout(callback, delay) {
  return 'local-timer:' + delay;
}
console.log(setTimeout(() => {}, 5));
const fetch = (url) => 'local-fetch:' + url;
console.log(fetch('/y'));
class Headers {
  constructor() {
    this.tag = 'local-headers';
  }
}
console.log(new Headers().tag);
class Map {
  set(key, value) {
    this.last = key + '=' + value;
    return this;
  }
}
const m = new Map();
console.log(m.set('a', 1).last);
const Symbol = { iterator: 'local-symbol' };
console.log(Symbol.iterator);
{
  const Math = { PI: 'local-pi', max: () => 'local-max' };
  console.log(Math.PI, Math.max(1, 2));
}
{
  const Number = { EPSILON: 'local-epsilon' };
  console.log(Number.EPSILON);
}
{
  const JSON = { stringify: () => 'local-stringify' };
  console.log(JSON.stringify({}));
}
{
  class Promise {
    constructor(executor) {
      this.tag = 'local-promise';
      executor();
    }
  }
  console.log(new Promise(() => {}).tag);
}
{
  class Proxy {
    constructor(target) {
      this.wrapped = target;
    }
  }
  console.log(new Proxy('t').wrapped);
}
{
  class RegExp {
    constructor(pattern) {
      this.pattern = pattern;
    }
  }
  console.log(new RegExp('abc').pattern);
}
console.log(Math.max(3, 4), typeof Number.EPSILON, JSON.stringify([1]));
