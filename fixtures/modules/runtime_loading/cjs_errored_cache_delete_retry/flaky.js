const state = require('./state' + '.js');
state.count++;

exports.partial = 'stale-partial';

if (state.count === 1) {
  throw new Error('boom-first');
}

exports.partial = undefined;
exports.done = 'ok-after-retry';
