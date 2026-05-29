// Class instance getter.
class Rectangle {
  constructor(w, h) {
    this.w = w;
    this.h = h;
  }
  get area() {
    return this.w * this.h;
  }
}

const r = new Rectangle(3, 4);
console.log(r.area);
