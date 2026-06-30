// #44：两个模块都导出 x，导入方分别取别名 xA/xB，应解析到各自来源。
import { x as xA } from './a.js';
import { x as xB } from './b.js';
console.log(xA);
console.log(xB);
