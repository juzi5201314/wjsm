// Basic inheritance via extends — documents current prototype chain behavior.
class Animal {
  constructor(name) {
    this.name = name;
  }
  speak() {
    return this.name + " makes sound";
  }
}

class Dog extends Animal {
  speak() {
    return super.speak() + " (bark)";
  }
}

const d = new Dog("Rex");
console.log(d.name);
console.log(d.speak());
console.log(d instanceof Animal);
console.log(d instanceof Dog);
