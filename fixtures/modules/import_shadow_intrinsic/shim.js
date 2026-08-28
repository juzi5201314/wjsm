export function fetch(url) {
  return 'shim-fetch:' + url;
}
export class Headers {
  constructor() {
    this.tag = 'shim-headers';
  }
}
export function structuredClone(value) {
  return 'shim-clone:' + value;
}
