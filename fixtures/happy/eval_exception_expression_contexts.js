function marker(name) {
  console.log("bad " + name);
  return true;
}

function Marker(name) {
  console.log("bad new " + name);
}

try {
  if (eval("throw 'if'")) {
    marker("if");
  }
} catch (e) {
  console.log(e);
}

try {
  eval("throw 'seq'"), marker("seq");
} catch (e) {
  console.log(e);
}

try {
  marker(eval("throw 'arg'"));
} catch (e) {
  console.log(e);
}

try {
  eval("throw 'binary'") + marker("binary");
} catch (e) {
  console.log(e);
}

try {
  new Marker(eval("throw 'new'"));
} catch (e) {
  console.log(e);
}

try {
  eval(eval("throw 'nested'"));
} catch (e) {
  console.log(e);
}
