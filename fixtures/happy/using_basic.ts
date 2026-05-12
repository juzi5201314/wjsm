function makeResource(value: number) {
  return { value };
}

using x = makeResource(42);
console.log(x.value);
