// Timer id captured in closure, cleared from different execution context.
console.log("start");

let capturedId;

function schedule() {
  capturedId = setTimeout(() => {
    console.log("SHOULD-NOT-RUN");
  }, 0);
}

schedule();

function cancelFromElsewhere() {
  clearTimeout(capturedId);
}

cancelFromElsewhere();

console.log("end");
