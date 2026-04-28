let i = 0;
while (i < 5) {
  i = i + 1;
  switch (i) {
    case 2:
      continue;
    case 4:
      continue;
  }
  console.log(i);
}
