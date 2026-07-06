console.log(typeof TextEncoder, typeof TextDecoder, typeof structuredClone, typeof queueMicrotask, typeof atob, typeof btoa, typeof performance.now);
console.log(new TextDecoder().decode(new TextEncoder().encode('hé')));
console.log(btoa('hi'), atob('aGk='));
let order = [];
order.push('sync');
queueMicrotask(() => order.push('qm'));
Promise.resolve().then(() => {
  order.push('promise');
  console.log(order.join(','));
});
console.log(Number.isFinite(performance.now()) && performance.now() >= 0);
