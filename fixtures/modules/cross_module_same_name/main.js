// #43：两个模块都声明顶层 const config，应各自独立，不冲突。
import { config as configA, readShared as readSharedA } from './a.js';
import { config as configB, readShared as readSharedB } from './b.js';
console.log(configA.value);
console.log(configB.value);
console.log(readSharedA());
console.log(readSharedB());
