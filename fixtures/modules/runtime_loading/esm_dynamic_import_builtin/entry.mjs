export function resolved() {
  return import.meta.resolve('node:path');
}

export function loadPath() {
  const specifier = 'node:path';
  return import(specifier);
}
