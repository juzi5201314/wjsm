try {
  require('./optional-missing.js');
  console.log('unexpected-load');
} catch (error) {
  console.log(error.name);
  console.log(error.message.includes("Cannot find module './optional-missing.js'"));
}
console.log('fallback-continued');
