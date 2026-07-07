const mjsTarget = './explicit.mjs';
import(mjsTarget)
    .then(() => import('./pkg/' + 'module.js'))
    .then(() => console.log('done'));
