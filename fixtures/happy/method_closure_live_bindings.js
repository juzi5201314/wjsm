let getterValue = 1;
const getterObject = {
  get value() {
    return getterValue;
  }
};
getterValue = 2;
console.log(getterObject.value === 2);

let setterValue = 1;
const setterObject = {
  set value(next) {
    setterValue = next;
  }
};
setterObject.value = 2;
console.log(setterValue === 2);

let methodValue = 1;
const methodObject = {
  update(next) {
    methodValue = next;
  }
};
methodObject.update(2);
console.log(methodValue === 2);

let homeValue = 1;
const homeBase = {
  update() {
    homeValue += 1;
    return homeValue;
  }
};
const homeObject = {
  __proto__: homeBase,
  update() {
    homeValue += 2;
    return super.update();
  }
};
homeValue = 10;
console.log(homeObject.update() === 13 && homeValue === 13);

let staticValue = 1;
class StaticClosure {
  static {
    staticValue = 2;
  }

  static read() {
    return staticValue;
  }
}
console.log(staticValue === 2 && StaticClosure.read() === 2);

let accessorValue = 1;
class AccessorClosure {
  get value() {
    return accessorValue;
  }

  set value(next) {
    accessorValue = next;
  }
}
const accessorWriter = new AccessorClosure();
const accessorReader = new AccessorClosure();
accessorWriter.value = 2;
console.log(accessorReader.value === 2 && accessorValue === 2);

let instanceMethodValue = 1;
class MethodClosure {
  update(next) {
    instanceMethodValue = next;
  }
}
new MethodClosure().update(2);
console.log(instanceMethodValue === 2);

let generatorValue = 1;
class GeneratorClosure {
  *read() {
    yield generatorValue;
  }
}
const generator = new GeneratorClosure().read();
generatorValue = 2;
console.log(generator.next().value === 2);
