// Test Proxy construct trap receives proxy as newTarget
function Target(x) {
  this.x = x;
}

let capturedNewTarget = null;
const handler = {
  construct(target, args, newTarget) {
    capturedNewTarget = newTarget;
    return Reflect.construct(target, args, newTarget);
  }
};

const proxy = new Proxy(Target, handler);
const instance = new proxy(42);

console.log(capturedNewTarget === proxy);
