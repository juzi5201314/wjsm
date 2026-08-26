// 静态键对象字面量模板化：长键、非 ASCII 键、数值键与超限属性。
const long = { firstName: 1, lastName: 2, quantity: 3 };
console.log("long dot:", long.firstName, long.lastName, long.quantity);
console.log("long bracket:", long["firstName"], long["lastName"]);
console.log("long keys:", JSON.stringify(Object.keys(long)));
console.log("long json:", JSON.stringify(long));
delete long.quantity;
console.log("long after delete:", JSON.stringify(Object.keys(long)));
console.log("long hasOwn:", long.hasOwnProperty("firstName"), long.hasOwnProperty("quantity"));

const unicode = { café: 1, 日本語: 2 };
console.log("unicode read:", unicode["café"], unicode["日本語"]);
console.log("unicode json:", JSON.stringify(unicode));
console.log("unicode key len:", Object.keys(unicode)[0].length);

const numeric = { 3: "a", 1e21: 1, 1.5: 1 };
console.log("numeric read:", numeric[3], numeric["1e21"], numeric[1.5]);
console.log("numeric keys:", JSON.stringify(Object.keys(numeric)));

function get(o, k) {
  return o[k];
}
const many = {
  a1: 1,
  a2: 2,
  a3: 3,
  a4: 4,
  a5: 5,
  a6: 6,
  a7: 7,
  a8: 8,
  a9: 9,
  a10: 10,
  a11: 11,
  a12: 12,
  a13: 13,
  a14: 14,
  a15: 15,
  a16: 16,
  a17: 17,
  a18: 18,
};
console.log("many dynamic:", get(many, "a17"), get(many, "a18"), get(many, "a1"));

const proto = { __proto__: { inherited: 1 }, own: 2 };
console.log("proto own:", proto.own, proto.inherited);

const computedKey = "dyn";
const computed = { [computedKey]: 42 };
console.log("computed:", computed.dyn);

const accessor = {
  get x() {
    return 7;
  },
};
console.log("getter:", accessor.x);

const spread = { ...{ spreadKey: 99 }, extra: 1 };
console.log("spread:", spread.spreadKey, spread.extra);

const frozen = { frozenKey: 1 };
Object.freeze(frozen);
let freezeError = "none";
try {
  frozen.frozenKey = 2;
} catch (error) {
  freezeError = error.name;
}
console.log("frozen value:", frozen.frozenKey, freezeError);
