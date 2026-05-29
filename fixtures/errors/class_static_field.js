// Static field — documents current behavior.
// If not supported, this fixture will be moved to errors or updated.
class Config {
  static version = "1.0.0";
  static getVersion() {
    return Config.version;
  }
}

console.log(Config.version);
console.log(Config.getVersion());
