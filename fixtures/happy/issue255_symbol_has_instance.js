// Symbol.hasInstance must be consulted before OrdinaryHasInstance (issue #255)
class Even {
  static [Symbol.hasInstance](x) {
    return x % 2 === 0;
  }
}
console.log(4 instanceof Even);
console.log(5 instanceof Even);

class Animal {}
class Dog extends Animal {}
const d = new Dog();
console.log(d instanceof Animal);
console.log(d instanceof Dog);