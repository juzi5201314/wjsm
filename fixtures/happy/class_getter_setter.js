// Property with both getter and setter — setter is invoked on assignment.
// The setter writes to `_celsius`, and the getter reads it back.
// The fahrenheit getter also reads `_celsius` and computes the conversion.
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
console.log(t._celsius);
