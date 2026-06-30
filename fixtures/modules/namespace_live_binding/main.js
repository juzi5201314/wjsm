// #45：命名空间对象的属性必须是 live binding——导出变量被改写后 ns.x 应反映新值。
import * as ns from './counter.js';
import { increment } from './counter.js';
console.log(ns.count);
increment();
console.log(ns.count);
