const letClosures = [];
for (let i = 0; i < 3; i++) {
  letClosures.push(() => i);
}
console.log(letClosures.map((read) => read()).join(","));

const varClosures = [];
for (var j = 0; j < 3; j++) {
  varClosures.push(() => j);
}
console.log(varClosures.map((read) => read()).join(","));

let whileValue = 0;
while (whileValue < 3) {
  const read = () => whileValue;
  whileValue++;
}
console.log("while=" + whileValue);

let closureWrite = 0;
while (closureWrite < 3) {
  (() => {
    closureWrite++;
  })();
}
console.log("closure-write=" + closureWrite);

function captureInsideFunction() {
  const closures = [];
  for (let i = 0; i < 3; i++) {
    closures.push(() => i);
  }
  return closures.map((read) => read()).join(",");
}
console.log("function=" + captureInsideFunction());

const nestedClosures = [];
for (let outer = 0; outer < 2; outer++) {
  for (let inner = 0; inner < 2; inner++) {
    nestedClosures.push(() => outer + ":" + inner);
  }
}
console.log(nestedClosures.map((read) => read()).join(","));

let createdBefore = 0;
const readCreatedBefore = () => createdBefore;
for (; createdBefore < 3; createdBefore++) {}
console.log("before=" + readCreatedBefore());

const nonLoopReads = [];
const nonLoopValue = 7;
for (let i = 0; i < 2; i++) {
  nonLoopReads.push(() => nonLoopValue);
}
console.log(nonLoopReads.map((read) => read()).join(","));

const keys = [];
for (let key in { a: 1, b: 2 }) {
  keys.push(() => key);
}
console.log("for-in=" + keys.map((read) => read()).join(","));

const values = [];
for (const value of [4, 5]) {
  values.push(() => value);
}
console.log("for-of=" + values.map((read) => read()).join(","));
