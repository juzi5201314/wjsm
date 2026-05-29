// Computed method name — documents actual behavior (gap: not supported).
// Computed method names produce undefined at runtime; the method is not installed on the prototype.
const methodName = "dynamic";

class Dynamic {
  [methodName]() {
    return "called";
  }
}

const d = new Dynamic();
console.log(typeof d.dynamic);
console.log(d.dynamic ? d.dynamic() : "no-dynamic-method");
