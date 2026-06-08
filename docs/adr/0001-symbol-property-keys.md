# ADR 0001: Symbol-capable property key encoding

## Status

Accepted

## Context

Object property slots stored `name_id` as a string-table offset. That made host-created well-known symbol properties impossible to represent as actual Symbol keys; using strings such as `"Symbol.iterator"` changed enumeration, descriptor, and Proxy trap behavior.

Arguments objects require `@@iterator` as a real `Symbol.iterator` key whose value is `%Array.prototype.values%`.

## Decision

Encode property slot `name_id` as a tagged `u32`:

- high bit clear: string key; low 31 bits are the string-table offset
- high bit set: Symbol key; low 31 bits are the symbol-table handle

The runtime exposes helpers for string and Symbol name-id encoding. Host property definition APIs have name-id and Symbol variants so runtime-created objects can install real Symbol keys. Enumeration APIs filter keys by type:

- `Object.keys`, `Object.values`, `Object.entries`, `Object.getOwnPropertyNames`, and `for...in` expose only string keys
- `Object.getOwnPropertySymbols` exposes only Symbol keys
- `Reflect.ownKeys` exposes string and Symbol keys
- Proxy traps receive Symbol values for Symbol-keyed operations

## Consequences

String-key name ids remain backward compatible because existing offsets have the high bit clear. Symbol-key lookup is still an integer comparison on the slot name id, matching string-key fast-path performance. String fallback comparison is skipped when either side is a Symbol key to avoid treating tagged symbol ids as memory offsets.

Array literal spread now expands spread elements through a host iterator path so `arguments` objects can be consumed by `[...arguments]` and `for...of` through the same observable iterator contract.
