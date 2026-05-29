// Static method — lives on constructor, not prototype.
class MathUtils {
  static square(x) {
    return x * x;
  }
}

console.log(MathUtils.square(4));
// Instance should not have it
const m = new MathUtils();
console.log(typeof m.square);
