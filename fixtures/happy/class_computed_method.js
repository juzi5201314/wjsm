// Computed method name — supported via lower_prop_name.
// Computed method names are lowered dynamically and installed on the prototype.
const methodName = "dynamic";

class Dynamic {
  [methodName]() {
    return "called";
  }
}

const d = new Dynamic();
console.log(typeof d.dynamic);
console.log(d.dynamic ? d.dynamic() : "no-dynamic-method");
