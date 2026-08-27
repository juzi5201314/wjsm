// 静态字段初始化器的 [[HomeObject]] 为构造器：super.x 沿父类构造器解析，
// super 方法调用以本类构造器为 this（ClassFieldDefinitionEvaluation）。
class Base {
  static kind = "base";
  static describe() {
    return "base-describe/" + this.extra;
  }
}
class Derived extends Base {
  static extra = "d";
  static fromSuperProp = super.kind + "!";
  static fromSuperCall = super.describe();
}
console.log(Derived.fromSuperProp);
console.log(Derived.fromSuperCall);

// static block 内的 super 与静态字段初始化器同一接线。
class Block extends Base {
  static extra = "blk";
  static {
    console.log("block:" + super.kind + "/" + super.describe());
  }
}

// 静态方法内的 super 不受影响（回归保护）。
class Method extends Base {
  static extra = "m";
  static run() {
    return super.describe();
  }
}
console.log(Method.run());
