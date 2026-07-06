import fs, { existsSync, readFileSync, statSync } from 'node:fs';
import fsp, { readFile } from 'node:fs/promises';
console.log(typeof fs.readFileSync, typeof readFileSync, typeof fsp.readFile, typeof readFile);
console.log(existsSync(__filename));
console.log(readFileSync(__filename, { encoding: 'utf8' }).includes('statSync'));
console.log(statSync(__filename).isFile());
const text = await readFile(__filename, 'utf8');
console.log(text.includes('node:fs/promises'));
