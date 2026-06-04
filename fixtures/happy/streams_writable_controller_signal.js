// Test: WritableStreamDefaultController.signal is an AbortSignal-like getter
let savedSignal;
const stream = new WritableStream({
  start(controller) {
    savedSignal = controller.signal;
    console.log("controller signal type:", typeof controller.signal);
    console.log("controller signal aborted:", controller.signal.aborted);
  }
});
console.log("saved signal type:", typeof savedSignal);
console.log("stream locked:", stream.locked);
