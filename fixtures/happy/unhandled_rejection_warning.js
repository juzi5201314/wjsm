// Issue #164: unhandled rejection warnings must show readable reasons.
Promise.reject(new Error("boom"));
Promise.reject("plain");