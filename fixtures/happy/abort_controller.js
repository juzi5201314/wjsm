// AbortController 基本语义：signal 对象、abort() 置位、缺省 reason 的
// AbortError 可观察字段、显式 reason 透传、重复 abort 无操作。
// 输出与 Node v22 逐字节对拍。
const controller = new AbortController();
console.log(typeof controller.signal);
console.log(controller.signal === controller.signal);
console.log(controller.signal.aborted);
console.log(controller.signal.reason);

controller.abort();
console.log(controller.signal.aborted);
console.log(controller.signal.reason.name);
console.log(controller.signal.reason.message);

// 重复 abort：已 aborted 时 reason 不再变化
const first = controller.signal.reason;
controller.abort("second");
console.log(controller.signal.reason === first);

// 显式 reason 原样透传（含非 Error 值）
const custom = new AbortController();
custom.abort("stop it");
console.log(custom.signal.aborted, custom.signal.reason);

const objectReason = new AbortController();
const marker = { code: 42 };
objectReason.abort(marker);
console.log(objectReason.signal.reason === marker);

// abort(undefined) 与缺省一致：合成 AbortError
const defaulted = new AbortController();
defaulted.abort(undefined);
console.log(defaulted.signal.reason.name);
