import('./dyn_mod.js').then(ns => {
  console.log(ns.x);
  console.log(ns.y);
});
import('./dyn_mod2.js').then(ns2 => {
  console.log(ns2.value);
});
