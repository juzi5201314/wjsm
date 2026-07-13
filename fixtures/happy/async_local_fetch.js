const { AsyncLocalStorage } = require('node:async_hooks');

const als = new AsyncLocalStorage();
als.run('fetch-context', () => {
  fetch('data:text/plain,ok')
    .then((response) => response.text())
    .then((text) => console.log(text, als.getStore()));
});
