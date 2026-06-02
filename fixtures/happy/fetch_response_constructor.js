// fetch_response_constructor
// new Response(), new Response(body), new Response(body, init) 
// status, ok, statusText, headers, bodyUsed lifecycle, text()

  const r1 = new Response();
  console.log("r1 status:", r1.status);
  console.log("r1 ok:", r1.ok);
  console.log("r1 statusText:", JSON.stringify(r1.statusText));
  console.log("r1 bodyUsed init:", r1.bodyUsed);

  const r2 = new Response("hello", { status: 201, statusText: "Created", headers: { "X-C": "1" } });
  console.log("r2 status:", r2.status);
  console.log("r2 ok:", r2.ok);
  console.log("r2 statusText:", JSON.stringify(r2.statusText));
  console.log("r2 has header:", r2.headers.has("x-c"));

  const txt = await r2.text();
  console.log("r2 text:", txt);
  console.log("r2 bodyUsed after consume:", r2.bodyUsed);

  // null body status with body tested in error fixture

  console.log("done response ctor");
