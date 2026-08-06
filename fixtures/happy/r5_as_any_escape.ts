// TS 注解在运行时被完全擦除：经 `as unknown as T` 逃逸后，实际值类型可与注解
// 不符。任何基于注解的优化都必须保持这一语义——本 fixture 是其可观测证明。
//
// 这里刻意大量使用不受检强制转换：被测对象正是「类型撒谎时运行时是否仍正确」，
// 逃逸口本身就是测试输入。全部写成 `as unknown as T`，不留裸 `any`。

class Point {
  x: number;
  y: number;
  constructor(x: number, y: number) {
    this.x = x;
    this.y = y;
  }
}

const p = new Point(1, 2);
console.log(p.x + p.y);

// 1. 注解外的属性：逃逸后新增属性，shape 改变，读写仍须正确
const loose = p as unknown as Record<string, unknown>;
loose.z = 30;
console.log(loose.z, p.x, p.y);
console.log(JSON.stringify(Object.keys(p)));

// 2. 注解声称 number 的字段被写入字符串：加法须走字符串拼接语义
loose.x = "a";
console.log(p.x + p.y);

// 3. delete 掉注解字段：读回 undefined，其余字段不受影响
delete loose.y;
console.log(p.y, p.x, loose.z);

// 4. 形参注解 number，实参却是字符串/布尔/null：须完全按 JS 语义求值
function add1(v: number): number {
  return v + 1;
}
const erased = add1 as unknown as (v: unknown) => unknown;
console.log(add1(1), erased("a"), erased(true), erased(null), erased(undefined));

// 5. 返回值注解 number，实际返回字符串
function claimsNumber(flag: boolean): number {
  return (flag ? "s" : 1) as unknown as number;
}
console.log(claimsNumber(true), claimsNumber(false), claimsNumber(true) + 1);

// 6. 注解为 class 实例，实际传入普通字面量对象（结构相同但 shape 不同）
function readX(o: Point): unknown {
  return o.x;
}
const fake = { x: "not-a-number", y: 0 } as unknown as Point;
console.log(readX(p), readX(fake));

// 7. 注解为 class 实例，实际传入缺字段的对象
const missing = {} as unknown as Point;
console.log(readX(missing));
