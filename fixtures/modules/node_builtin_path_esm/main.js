import path, { basename, posix, win32 } from 'node:path';
console.log(path.join('a', '', 'b'));
console.log(basename('/tmp/file.txt', '.txt'));
console.log(posix.relative('/a/b', '/a/c/d'));
console.log(win32.basename('C:\\tmp\\file.txt'));
