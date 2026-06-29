// #144: break 目标仍在 try 内部时，该 try 的 finally 不应在 break 时执行
try {
  for (let i = 0; i < 10; i++) {
    if (i === 5) break;
  }
  console.log("after loop");
} finally {
  console.log("finally");
}
