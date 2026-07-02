// Private fields are not ordinary string-keyed properties.
// The declared #x slot stays accessible inside the class only; a public "#x"
// property can coexist without reading or overwriting the private slot.
class SecretBox {
  #x = 42;

  getX() {
    return this.#x;
  }
}

const box = new SecretBox();
console.log(box.getX());
console.log(box["#x"] === undefined);
console.log(Object.getOwnPropertyNames(box).join("|") || "none");
console.log(Reflect.ownKeys(box).join("|") || "none");
console.log(JSON.stringify(box));

box["#x"] = 7;
console.log(box.getX());
console.log(box["#x"]);
console.log(Object.keys(box).join("|"));
