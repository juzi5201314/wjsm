// Test: Response.clone() creates an independently consumable stream body snapshot
const res = new Response("clone-stream");
const clone = res.clone();
console.log("orig body object:", typeof res.body);
console.log("clone body object:", typeof clone.body);
console.log("orig bodyUsed before:", res.bodyUsed);
console.log("clone bodyUsed before:", clone.bodyUsed);
const origReader = res.body.getReader();
console.log("orig bodyUsed after getReader:", res.bodyUsed);
console.log("clone bodyUsed after orig getReader:", clone.bodyUsed);
const origFirst = await origReader.read();
console.log("orig first length:", origFirst.value.length);
const cloneReader = clone.body.getReader();
console.log("clone bodyUsed after getReader:", clone.bodyUsed);
cloneReader.read().then(cloneFirst => {
  console.log("clone first length:", cloneFirst.value.length);
});
