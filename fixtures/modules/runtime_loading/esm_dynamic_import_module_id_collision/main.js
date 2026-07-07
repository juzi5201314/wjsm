const firstSpecifier = './first/entry.mjs';
const secondSpecifier = './second/entry.mjs';

import(firstSpecifier)
  .then(first => import(secondSpecifier).then(second => ({ first, second })))
  .then(({ first, second }) => first.load().then(value => ({ firstValue: value, second })))
  .then(({ firstValue, second }) => {
    console.log(firstValue);
    return second.load();
  })
  .then(secondValue => console.log(secondValue));
