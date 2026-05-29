// Property with both getter and setter — documents actual behavior.
// NOTE: Setter is currently bypassed; assignment creates own data property.
// The getter reads the own property so t.celsius appears to work, but _celsius
// (which the setter should write to) stays at 0.
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
