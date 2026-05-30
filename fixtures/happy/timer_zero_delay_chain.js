// Chained setTimeout calls
const state = { count: 0 };

setTimeout(() => {
  state.count++;
  console.log("chain-1:", state.count);
  setTimeout(() => {
    state.count++;
    console.log("chain-2:", state.count);
    setTimeout(() => {
      state.count++;
      console.log("chain-3:", state.count);
    }, 0);
  }, 0);
}, 0);

console.log("main");
