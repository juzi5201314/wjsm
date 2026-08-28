// 含 top-level await 的主脚本抛出的异常经 main promise rejection 报告并
// 以非零码退出，不得静默退出 0。
await Promise.resolve();
throw new Error("tla boom");
