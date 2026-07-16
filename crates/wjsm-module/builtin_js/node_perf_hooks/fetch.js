// Fetch Resource Timing 由 host fetch owner 采集；加载 perf_hooks 不得改写 Web API identity。

var installFetch;

function loadPerfHooksFetch() {
function installFetch() {}

return { installFetch: installFetch };
}

const perfHooksFetch = loadPerfHooksFetch();
installFetch = perfHooksFetch.installFetch;
