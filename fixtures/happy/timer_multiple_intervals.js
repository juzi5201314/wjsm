// Multiple intervals with different frequencies
const state = { fast: 0, slow: 0 };

state.fastId = setInterval(() => {
  state.fast++;
  console.log("fast:", state.fast);
  if (state.fast >= 3) {
    clearInterval(state.fastId);
    clearInterval(state.slowId);
  }
}, 0);

state.slowId = setInterval(() => {
  state.slow++;
  console.log("slow:", state.slow);
}, 100);

console.log("main");
