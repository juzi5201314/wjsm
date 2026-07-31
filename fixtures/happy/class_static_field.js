// Static field — static members reference the class name (initialised after class evaluation).
class Config {
  static version = "1.0.0";
  static getVersion() {
    return Config.version;
  }
}

console.log(Config.version);
console.log(Config.getVersion());
