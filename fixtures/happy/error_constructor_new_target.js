const direct = new TypeError("direct");
console.log(Object.getPrototypeOf(direct) === TypeError.prototype);
console.log(Object.getPrototypeOf(TypeError.prototype) === Error.prototype);

const called = TypeError("called");
console.log(Object.getPrototypeOf(called) === TypeError.prototype);

console.log(Reflect.get(Error, "prototype") === Error.prototype);
console.log(Reflect.get(TypeError, "prototype") === TypeError.prototype);

const range = new RangeError("range");
console.log(Object.getPrototypeOf(range) === RangeError.prototype);
console.log(Object.getPrototypeOf(RangeError.prototype) === Error.prototype);

class CustomTypeError extends TypeError {}
const derived = new CustomTypeError("derived");
console.log(Object.getPrototypeOf(derived) === CustomTypeError.prototype);
console.log(Object.getPrototypeOf(Object.getPrototypeOf(derived)) === TypeError.prototype);
console.log(derived instanceof CustomTypeError);
console.log(derived instanceof TypeError);
console.log(derived instanceof Error);
console.log(derived.name);
console.log(derived.message);
console.log(derived.toString());

class ExplicitTypeError extends TypeError {
  constructor(message) {
    const result = super(message);
    console.log(result === this);
  }
}
const explicit = new ExplicitTypeError("explicit");
console.log(Object.getPrototypeOf(explicit) === ExplicitTypeError.prototype);
console.log(explicit instanceof TypeError);
console.log(explicit.message);

function ReflectNewTarget() {}
ReflectNewTarget.prototype = { marker: 1 };
const reflected = Reflect.construct(TypeError, ["reflected"], ReflectNewTarget);
console.log(Object.getPrototypeOf(reflected) === ReflectNewTarget.prototype);
console.log(reflected.message);
