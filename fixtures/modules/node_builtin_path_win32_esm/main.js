import win32, { basename, extname, isAbsolute, sep, delimiter } from 'node:path/win32';
import win32Bare from 'path/win32';
import path from 'node:path';
console.log(win32 === win32Bare);
console.log(win32 === path.win32);
console.log(basename('C:\\temp\\data.txt'), extname('C:\\temp\\data.txt'));
console.log(isAbsolute('C:\\x'), isAbsolute('x'));
console.log(sep, delimiter);
