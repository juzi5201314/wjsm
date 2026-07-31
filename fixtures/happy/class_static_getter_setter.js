// Static getter/setter.
class Settings {
  static #mode = "dark";
  
  static get mode() {
    return Settings.#mode;
  }
  
  static set mode(v) {
    Settings.#mode = v;
  }
}

console.log(Settings.mode);
Settings.mode = "light";
console.log(Settings.mode);
