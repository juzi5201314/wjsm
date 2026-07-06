import fs, { existsSync, readFileSync, statSync } from 'node:fs';
import fsp, { readFile } from 'node:fs/promises';
console.log(typeof fs.readFileSync, typeof readFileSync, typeof fsp.readFile, typeof readFile);
console.log(existsSync(import.meta.filename));
console.log(readFileSync(import.meta.filename, { encoding: 'utf8' }).includes('statSync'));
console.log(statSync(import.meta.filename).isFile());
const text = await readFile(import.meta.filename, 'utf8');
console.log(text.includes('node:fs/promises'));
