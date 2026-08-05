// case A: object-local closing - new_object + set x/y + read x (sa-replace)
function f() { const o = { x: 1, y: 2 }; return o.x; }
console.log(f()); // 1
// case B: escaping into another object
const leak = {};
function g() { const o = { x: 3 }; leak.inner = o; return 9; }
console.log(g(), leak.inner.x); // 9 3
// case C: return escape
function h() { const o = { x: 1 }; return o; }
console.log(h().x); // 1