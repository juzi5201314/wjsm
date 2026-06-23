// SetProto IR 与 Object.setPrototypeOf：原型为 Proxy 时必须识别 TAG_PROXY（issue #27）
const proto = { label: "proto" };
const proxied = new Proxy(proto, {});

const viaSetProto = {};
viaSetProto.__proto__ = proxied;
console.log("ir_proto_label:", viaSetProto.label);

const viaBuiltin = {};
const ret = Object.setPrototypeOf(viaBuiltin, proxied);
console.log("builtin_ret_is_target:", ret === viaBuiltin);
console.log("builtin_proto_label:", viaBuiltin.label);