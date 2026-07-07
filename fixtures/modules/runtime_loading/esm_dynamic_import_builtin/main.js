const entrySpecifier = './entry.mjs';

console.log(require.resolve('node:path'));
import(entrySpecifier)
  .then(entry => {
    console.log(entry.resolved());
    return entry.loadPath();
  })
  .then(path => {
    console.log(path.join('a', 'b'));
    console.log(path.default.join('c', 'd'));
  });
