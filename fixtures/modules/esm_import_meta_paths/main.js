export {};
console.log(import.meta.url.startsWith('file://'));
console.log(import.meta.url.endsWith('/fixtures/modules/esm_import_meta_paths/main.js'));
console.log(import.meta.filename.endsWith('/fixtures/modules/esm_import_meta_paths/main.js'));
console.log(import.meta.dirname.endsWith('/fixtures/modules/esm_import_meta_paths'));
