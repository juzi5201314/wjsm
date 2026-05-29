// Static block — executes during class definition.
class WithStaticBlock {
  static {
    console.log("static-block-executed");
  }
  constructor() {
    console.log("ctor");
  }
}

console.log("after-class-def");
new WithStaticBlock();
