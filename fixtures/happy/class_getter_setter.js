// Property with both getter and setter.
class Temperature {
  constructor() {
    this._celsius = 0;
  }
  get celsius() { return this._celsius; }
  set celsius(v) { this._celsius = v; }
  get fahrenheit() { return this._celsius * 9 / 5 + 32; }
}

const t = new Temperature();
t.celsius = 100;
console.log(t.celsius);
console.log(t.fahrenheit);
