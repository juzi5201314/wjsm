// Updating a binding after a conditional capture uses the local value when no closure was created.
function falseCase() {
  let disabled = false;
  let x = 0;
  if (disabled) {
    (() => x);
  }
  x++;
  console.log(x);
}

// The same update must still reach the shared env when the closure exists.
function trueCase() {
  let enabled = true;
  let read;
  let y = 0;
  if (enabled) {
    read = () => y;
  }
  y++;
  console.log(read());
}

falseCase();
trueCase();
