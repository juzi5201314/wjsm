const fn = async function(x) {
  return x * 2;
};

fn(7).then(v => console.log(v));
