let x = 0;

async function inc() {
  x = x + 1;
}

inc().then(() => console.log(x));
