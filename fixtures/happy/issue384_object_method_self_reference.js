// GitHub #384：直接对象字面量 initializer 的延迟方法/访问器体可以引用
// 正在初始化该对象的当前 let/const binding（只有 let 可赋值或更新）。
const set = {
  forEach(action) {
    action(set);
  },
  method() {
    return set;
  },
  get self() {
    return set;
  },
  set self(value) {
    this.setterSawSelf = value === set;
  },
  nested() {
    return {
      method() {
        return set;
      },
      get self() {
        return set;
      },
    };
  },
};

set.forEach((value) => console.log(typeof value));
console.log(set.method() === set);
console.log(set.self === set);
set.self = set;
console.log(set.setterSawSelf);
console.log(set.nested().method() === set);
console.log(set.nested().self === set);

let mutable = {
  get self() {
    return mutable;
  },
};
console.log(mutable.self === mutable);

let assigned = {
  assign(value) {
    assigned = value;
  },
};
const assignOwner = assigned;
const replacement = { replaced: true };
assignOwner.assign(replacement);
console.log(assigned === replacement);

let updated = {
  update() {
    updated = 40;
    updated++;
    return updated;
  },
};
const updateOwner = updated;
console.log(updateOwner.update() === 41);
console.log(updated === 41);
