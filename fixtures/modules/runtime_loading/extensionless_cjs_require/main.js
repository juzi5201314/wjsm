let child;
if (true) {
    child = require('./child');
}
console.log(child.marker);
console.log(child.pathsIsArray);
console.log(child.firstPathIsLocal);
console.log(child.moduleBinding);
console.log(child.exportsBinding);
console.log(child.moduleExportsMatches);
