import posix, { join, basename, dirname, extname, isAbsolute, sep, delimiter } from 'node:path/posix';
import posixBare from 'path/posix';
import path from 'node:path';
console.log(posix === posixBare);
console.log(posix === path.posix);
console.log(join('a', 'b', '..', 'c'));
console.log(basename('/tmp/file.txt', '.txt'), dirname('/tmp/file.txt'), extname('/tmp/file.txt'));
console.log(isAbsolute('/x'), isAbsolute('x'));
console.log(sep, delimiter);
