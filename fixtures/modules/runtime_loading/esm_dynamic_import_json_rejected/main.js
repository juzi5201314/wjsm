const target = './data' + '.json';
import(target)
  .then(() => console.log('unexpected-json-import-success'))
  .catch(error => {
    console.log(error.message.indexOf('JSON import') !== -1);
    console.log(error.message.indexOf('import assertions') !== -1);
  });
