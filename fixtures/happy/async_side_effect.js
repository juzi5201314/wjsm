let x = 0;

async function inc() {
  await Promise.resolve(undefined);
  x = x + 1;
}

inc().then(() => console.log(x));
