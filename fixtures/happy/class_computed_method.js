// Computed method name — documents current behavior (may be gap or error).
const methodName = "dynamic";

class Dynamic {
  [methodName]() {
    return "called";
  }
}

const d = new Dynamic();
console.log(typeof d.dynamic);
console.log(d.dynamic ? d.dynamic() : "no-dynamic-method");
