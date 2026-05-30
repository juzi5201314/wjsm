// Interval clears itself from inside callback — documents actual behavior.
console.log("start");

const state = { count: 0, id: null };
state.id = setInterval(() => {
  state.count++;
  console.log("tick", state.count);
  if (state.count >= 1) {
    clearInterval(state.id);
  }
}, 0);

console.log("end");
