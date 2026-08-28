// 条件与 callee 操作数位置的表达式级异常分叉：getter/Proxy 陷阱抛出必须
// 先于 ToBoolean 分支 / 调用本身传播（进入本地 try/catch），异常哨兵不得
// 被 Branch 当真值消费（do-while 曾因此死循环）、不得作为 callee 流入
// Call/Construct（曾误报 "... is not a function" 并丢失原始异常）。
// 同步与 async 状态机体各跑一遍，行为与 Node 一致。

function run(label, body) {
  try {
    body();
    console.log(label + " not-thrown");
  } catch (e) {
    console.log(label + " caught: " + e.message);
  }
}

const boom = (msg) => ({
  get t() {
    throw new Error(msg);
  },
});

function suite(prefix) {
  run(prefix + "cond-test", () => {
    const r = boom("cond").t ? "x" : "y";
    console.log("unreachable " + r);
  });
  run(prefix + "while-test", () => {
    while (boom("while").t) {
      break;
    }
  });
  run(prefix + "dowhile-test", () => {
    let n = 0;
    do {
      n++;
    } while (boom("dowhile-n" + n).t);
  });
  run(prefix + "for-test", () => {
    for (; boom("fortest").t; ) {
      break;
    }
  });
  run(prefix + "for-init", () => {
    for (boom("forinit").t; false; ) {}
  });
  run(prefix + "for-update", () => {
    for (let i = 0; i < 2; boom("forupdate").t) {
      i++;
    }
  });
  run(prefix + "call-callee", () => {
    const o = {
      get m() {
        throw new Error("callee");
      },
    };
    o.m();
  });
  run(prefix + "computed-callee", () => {
    const o = {
      get m() {
        throw new Error("computed-callee");
      },
    };
    o["m"]();
  });
  run(prefix + "opt-callee", () => {
    const o = {
      get m() {
        throw new Error("opt-callee");
      },
    };
    o?.m();
  });
  run(prefix + "optcall-callee", () => {
    const o = {
      get m() {
        throw new Error("optcall");
      },
    };
    o.m?.();
  });
  run(prefix + "tagged-callee", () => {
    const o = {
      get tag() {
        throw new Error("tagged");
      },
    };
    o.tag`x`;
  });
  run(prefix + "new-callee", () => {
    const o = {
      get C() {
        throw new Error("new-callee");
      },
    };
    new o.C();
  });
}

// 可选调用成员路径的 receiver 只求值一次（f().m?.() 不得二次求值）。
let receiverEvals = 0;
const mkReceiver = () => {
  receiverEvals++;
  return {
    m() {
      return 42;
    },
  };
};
console.log("optcall value: " + mkReceiver().m?.());
console.log("receiver evals: " + receiverEvals);

suite("sync ");

async function main() {
  suite("async ");
  // 未捕获的条件位置抛出必须 reject async 函数返回的 promise。
  const rejected = await (async () => {
    while (boom("uncaught-loop").t) {
      break;
    }
  })().then(
    () => "resolved",
    (e) => "rejected: " + e.message
  );
  console.log("async promise " + rejected);
  console.log("main done");
}

main();
