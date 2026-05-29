// Timer inside async function — verifies interaction with async/await.
async function demo() {
  console.log("async-start");
  await Promise.resolve();
  console.log("after-await");
  
  setTimeout(() => {
    console.log("timeout-in-async");
  }, 0);
  
  console.log("async-end");
}

demo();
console.log("main-after-async-call");
