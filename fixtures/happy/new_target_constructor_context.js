let seen;

function Check() {
  seen = new.target;
}

function Inner() {
  console.log(new.target === undefined);
}

function Plain() {
  console.log(new.target === undefined);
}

function Outer() {
  console.log(new.target === undefined);
  Plain();
  new Inner();
  console.log(new.target === undefined);
}

Check();
console.log(seen === undefined);
new Check();
console.log(seen === Check);

Outer();
new Outer();
