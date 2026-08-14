// 21 个 typed native math thunk 的边界用例：NaN / ±0 / ±Infinity / 定义域外。
// 这些实参都是数字字面量或 f64 算术表达式（0/0、1/0、-0），infer_f64_values
// 会证明为 f64，因此走 typed native direct call；dispatcher 路径由其它 fixture 覆盖。
// 对结果可能为 -0 的函数额外输出 1/结果，以区分 +0 / -0。
console.log(Math.acos(0.5), Math.acos(-0.5), Math.acos(1), Math.acos(-1), Math.acos(0), Math.acos(-0), Math.acos(2), Math.acos(-2), Math.acos(1 / 0), Math.acos(0 / 0));
console.log(Math.acosh(1), Math.acosh(0), Math.acosh(-0), Math.acosh(0 / 0), Math.acosh(1 / 0));
console.log(Math.asin(0.5), Math.asin(-0.5), Math.asin(1), Math.asin(-1), Math.asin(0), Math.asin(-0), Math.asin(2), Math.asin(-2), Math.asin(1 / 0), Math.asin(0 / 0), 1 / Math.asin(0), 1 / Math.asin(-0));
console.log(Math.asinh(0), Math.asinh(-0), Math.asinh(1 / 0), Math.asinh(-1 / 0), Math.asinh(0 / 0), 1 / Math.asinh(0), 1 / Math.asinh(-0));
console.log(Math.atan(0), Math.atan(-0), Math.atan(1 / 0), Math.atan(-1 / 0), Math.atan(0 / 0), 1 / Math.atan(0), 1 / Math.atan(-0));
console.log(Math.atanh(0), Math.atanh(-0), Math.atanh(1), Math.atanh(-1), Math.atanh(2), Math.atanh(-2), Math.atanh(1 / 0), Math.atanh(0 / 0), 1 / Math.atanh(0), 1 / Math.atanh(-0));
console.log(Math.cbrt(0), Math.cbrt(-0), Math.cbrt(8), Math.cbrt(-8), Math.cbrt(1 / 0), Math.cbrt(-1 / 0), Math.cbrt(0 / 0), 1 / Math.cbrt(0), 1 / Math.cbrt(-0));
console.log(Math.cos(0), Math.cos(-0), Math.cos(1 / 0), Math.cos(-1 / 0), Math.cos(0 / 0));
console.log(Math.cosh(0), Math.cosh(-0), Math.cosh(1 / 0), Math.cosh(-1 / 0), Math.cosh(0 / 0));
console.log(Math.exp(0), Math.exp(-0), Math.exp(1 / 0), Math.exp(-1 / 0), Math.exp(0 / 0));
console.log(Math.expm1(0), Math.expm1(-0), Math.expm1(1 / 0), Math.expm1(-1 / 0), Math.expm1(0 / 0), 1 / Math.expm1(0), 1 / Math.expm1(-0));
console.log(Math.log(1), Math.log(0), Math.log(-0), Math.log(-1), Math.log(1 / 0), Math.log(-1 / 0), Math.log(0 / 0));
console.log(Math.log1p(0), Math.log1p(-0), Math.log1p(-1), Math.log1p(-2), Math.log1p(1 / 0), Math.log1p(0 / 0), 1 / Math.log1p(0), 1 / Math.log1p(-0));
console.log(Math.log10(1), Math.log10(0), Math.log10(-0), Math.log10(-1), Math.log10(1 / 0), Math.log10(0 / 0));
console.log(Math.log2(1), Math.log2(0), Math.log2(-0), Math.log2(-1), Math.log2(1 / 0), Math.log2(0 / 0));
console.log(Math.sin(0), Math.sin(-0), Math.sin(1 / 0), Math.sin(-1 / 0), Math.sin(0 / 0), 1 / Math.sin(0), 1 / Math.sin(-0));
console.log(Math.sinh(0), Math.sinh(-0), Math.sinh(1 / 0), Math.sinh(-1 / 0), Math.sinh(0 / 0), 1 / Math.sinh(0), 1 / Math.sinh(-0));
console.log(Math.tan(0), Math.tan(-0), Math.tan(1 / 0), Math.tan(-1 / 0), Math.tan(0 / 0), 1 / Math.tan(0), 1 / Math.tan(-0));
console.log(Math.tanh(0), Math.tanh(-0), Math.tanh(1 / 0), Math.tanh(-1 / 0), Math.tanh(0 / 0), 1 / Math.tanh(0), 1 / Math.tanh(-0));
console.log(Math.atan2(0, 0), Math.atan2(-0, 0), Math.atan2(0, -0), Math.atan2(-0, -0), Math.atan2(1, -0), Math.atan2(-1, -0), Math.atan2(0 / 0, 1), Math.atan2(1, 0 / 0), 1 / Math.atan2(0, 0), 1 / Math.atan2(-0, 0));
console.log(Math.pow(2, 3), Math.pow(-2, 3), Math.pow(-2, 2), Math.pow(-2, 0.5), Math.pow(-0, 3), Math.pow(-0, 2), Math.pow(-0, -3), Math.pow(-0, -2), 1 / Math.pow(-0, 3), 1 / Math.pow(-0, 2), Math.pow(0 / 0, 0), Math.pow(1, 0 / 0));
