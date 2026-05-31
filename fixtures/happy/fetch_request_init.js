// fetch_request_init
// new Request(input), new Request(input, init) covering method, headers, body, redirect, cache, credentials, integrity, keepalive
// also new Request(otherRequest) copy constructor

const r1 = new Request("data:text/plain,hello");
console.log("r1 method:", r1.method);
console.log("r1 url data:", r1.url.indexOf("data:") === 0);
console.log("r1 redirect:", r1.redirect);

const init = {
  method: "POST",
  headers: { "X-Custom": "val" },
  body: "payload",
  redirect: "manual",
  cache: "no-store",
  credentials: "omit",
  integrity: "sha256-xxx",
  keepalive: true,
};
const r2 = new Request("data:text/plain,post", init);
console.log("r2 method:", r2.method);
console.log("r2 has custom header:", r2.headers.has("x-custom"));
console.log("r2 redirect:", r2.redirect);
console.log("r2 cache:", r2.cache);
console.log("r2 credentials:", r2.credentials);
console.log("r2 integrity:", r2.integrity);
console.log("r2 keepalive:", r2.keepalive);

// copy constructor
const r3 = new Request(r2);
console.log("r3 method from copy:", r3.method);
console.log("r3 url same as r2:", r3.url === r2.url);
console.log("r3 headers independent:", r3.headers !== r2.headers);

// GET with body should error in error fixture; here just basic

console.log("done request ctor");
