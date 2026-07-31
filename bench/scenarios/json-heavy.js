// json-heavy: 64 字段嵌套对象序列化 + 反序列化
const WINDOW_MS = Number(process.env.BENCH_WINDOW_MS || 1000);
const WARMUP_MS = Number(process.env.BENCH_WARMUP_MS || 500);

const JSON_TEXT = '{"id":1,"name":"benchmark","active":true,"ratio":0.618,'
  + '"tags":["alpha","beta","gamma"],"nested":{"level":1,"value":42,'
  + '"deep":{"x":1.5,"y":-2.25,"z":3.375},"list":[1,2,3,4,5,6,7,8]},'
  + '"a1":1,"a2":2,"a3":3,"a4":4,"a5":5,"a6":6,"a7":7,"a8":8,'
  + '"b1":"one","b2":"two","b3":"three","b4":"four","b5":"five","b6":"six","b7":"seven","b8":"eight",'
  + '"c1":true,"c2":false,"c3":true,"c4":false,"c5":true,"c6":false,"c7":true,"c8":false,'
  + '"d1":1.1,"d2":2.2,"d3":3.3,"d4":4.4,"d5":5.5,"d6":6.6,"d7":7.7,"d8":8.8,'
  + '"e1":[1],"e2":[1,2],"e3":[1,2,3],"e4":[1,2,3,4],"e5":[1,2,3,4,5],'
  + '"e6":[1,2,3,4,5,6],"e7":[1,2,3,4,5,6,7],"e8":[1,2,3,4,5,6,7,8],'
  + '"f1":{"k":1},"f2":{"k":2},"f3":{"k":3},"f4":{"k":4},'
  + '"f5":{"k":5},"f6":{"k":6},"f7":{"k":7},"f8":{"k":8}}';

function work() {
  const parsed = JSON.parse(JSON_TEXT);
  return JSON.stringify(parsed).length;
}

for (const end = performance.now() + WARMUP_MS; performance.now() < end;) work();

let iterations = 0;
const t0 = performance.now();
while (performance.now() - t0 < WINDOW_MS) { work(); iterations++; }
console.log(`ns_per_op=${((performance.now() - t0) * 1e6 / Math.max(iterations, 1)).toFixed(1)} iterations=${iterations}`);
