function addOne(x) {
  return x + 1;
}

class Calc {
  pick(flag, base) {
    if (flag) {
      return base + 2;
    }
    return addOne(base);
  }
}

const calc = new Calc();
console.log(calc.pick(true, 10));
console.log(calc.pick(false, 10));
console.log(calc.pick(true, 20));
console.log(calc.pick(false, 20));