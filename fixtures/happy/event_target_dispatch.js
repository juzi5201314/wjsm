// EventTarget 通用派发语义：options 归一化、once、去重、派发中变更可见性、
// stopImmediatePropagation、preventDefault、handleEvent 对象监听器。
// 输出与 Node v22 逐字节对拍。
const et = new EventTarget();
const log = [];
const f = (e) => log.push("f:" + e.type);
et.addEventListener("x", f);
et.addEventListener("x", f); // 相同 (type, callback, capture) 去重
et.addEventListener("x", f, { capture: 1 }); // capture 真值化 → 不同键
et.dispatchEvent(new Event("x"));
console.log(JSON.stringify(log));

// removeEventListener 布尔第三参不匹配对象形式注册的 capture
et.removeEventListener("x", f, true);
log.length = 0;
et.dispatchEvent(new Event("x"));
console.log("after remove bool:", JSON.stringify(log));
et.removeEventListener("x", f, { capture: true });
log.length = 0;
et.dispatchEvent(new Event("x"));
console.log("after remove obj:", JSON.stringify(log));

// once：只触发一次即自动移除
const et2 = new EventTarget();
let onceRuns = 0;
et2.addEventListener("y", () => onceRuns++, { once: true });
et2.dispatchEvent(new Event("y"));
et2.dispatchEvent(new Event("y"));
console.log("once:", onceRuns);

// 派发中：移除后续监听器可见（不触发），新追加的监听器不参与本轮
const et3 = new EventTarget();
const seen = [];
const l2 = () => seen.push("l2");
et3.addEventListener("z", () => {
  seen.push("l1");
  et3.removeEventListener("z", l2);
  et3.addEventListener("z", () => seen.push("l3"));
});
et3.addEventListener("z", l2);
et3.dispatchEvent(new Event("z"));
console.log("mutation:", JSON.stringify(seen));
seen.length = 0;
et3.dispatchEvent(new Event("z"));
console.log("second round:", JSON.stringify(seen));

// stopImmediatePropagation 阻断后续监听器；preventDefault 使返回值为 false
const et4 = new EventTarget();
et4.addEventListener("w", (e) => {
  seen.push("w1");
  e.stopImmediatePropagation();
});
et4.addEventListener("w", () => seen.push("w2"));
seen.length = 0;
console.log("sip ret:", et4.dispatchEvent(new Event("w")), JSON.stringify(seen));
const cancelable = new Event("c", { cancelable: true });
et4.addEventListener("c", (e) => e.preventDefault());
console.log("cancel ret:", et4.dispatchEvent(cancelable), cancelable.defaultPrevented);

// handleEvent 对象监听器：this 绑定到对象自身
const et5 = new EventTarget();
const handler = {
  handleEvent(e) {
    seen.push("he:" + (this === handler) + ":" + e.type);
  },
};
et5.addEventListener("h", handler);
seen.length = 0;
et5.dispatchEvent(new Event("h"));
console.log("handleEvent:", JSON.stringify(seen));

// Event 对象基础字段与派发期状态
const ev = new Event("probe", { bubbles: true, cancelable: true, composed: true });
console.log(ev.type, ev.bubbles, ev.cancelable, ev.composed, ev.isTrusted, ev.target, ev.eventPhase);
console.log(typeof ev.timeStamp, Object.prototype.toString.call(ev));
const et6 = new EventTarget();
et6.addEventListener("probe", (e) => {
  console.log("during:", e.eventPhase, e.target === et6, e.currentTarget === et6);
});
et6.dispatchEvent(ev);
console.log("after:", ev.eventPhase, ev.target === et6, ev.currentTarget);
