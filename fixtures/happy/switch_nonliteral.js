function getValue() { return 2; }
var SOME_CONST = 3;

// Function call as case test
var x = 2;
switch (x) {
  case getValue():
    console.log("function call match");
    break;
  case 1:
    console.log("one");
    break;
  default:
    console.log("default");
}

// Variable reference as case test
switch (x) {
  case SOME_CONST:
    console.log("const match");
    break;
  case getValue():
    console.log("function call match 2");
    break;
  default:
    console.log("no match");
}

// String case comparison (strict equality, not raw pointer compare)
var s = "hello";
switch (s) {
  case "hello":
    console.log("string match");
    break;
  case "world":
    console.log("world");
    break;
  default:
    console.log("no string match");
}

// Computed expression as case test
switch (x) {
  case 1 + 1:
    console.log("computed match");
    break;
  default:
    console.log("default computed");
}
