// Duplicate private field names in same class — spec early error.
class Bad {
  #x = 1;
  #x = 2;  // duplicate
}
console.log(new Bad());
