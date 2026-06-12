var result;
function Ctor() {
    result = eval('new.target');
}
new Ctor();
console.log(typeof result);
console.log(result === Ctor);