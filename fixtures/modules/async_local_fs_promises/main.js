const fs = require('fs/promises');
const { AsyncLocalStorage } = require('async_hooks');
const als = new AsyncLocalStorage();
als.run('fs-context', () => {
  fs.readFile('Cargo.toml', 'utf8').then((source) => {
    console.log(source.length > 0, als.getStore());
    process.exit(0);
  });
});
