function replaceClass(value, context) {
  console.log(context.kind);
  console.log(context.name);
  return function() {
    this.label = "decorated";
  };
}

@replaceClass
class Example {}

console.log(new Example().label);

function replaceMethod(value, context) {
  console.log(context.kind);
  console.log(context.name);
  return function() {
    return "method decorated";
  };
}

class Worker {
  @replaceMethod
  work() {
    return "base";
  }
}

console.log(new Worker().work());
