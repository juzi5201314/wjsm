// readline 接任意事件流输入：跨块拼接（多字节字符与 \r\n 被劈开）、
// rl.write 注入、EOF 残余行交付。输出与 Node v22 逐字节一致（oracle 校验）。
import readline from 'node:readline';

function makeInput() {
  const listeners = { data: [], end: [] };
  return {
    on(event, listener) {
      if (listeners[event]) listeners[event].push(listener);
      return this;
    },
    removeListener(event, listener) {
      if (!listeners[event]) return this;
      const index = listeners[event].lastIndexOf(listener);
      if (index >= 0) listeners[event].splice(index, 1);
      return this;
    },
    resume() {
      return this;
    },
    pause() {
      return this;
    },
    emit(event, value) {
      const queue = listeners[event] ? listeners[event].slice() : [];
      for (let i = 0; i < queue.length; i = i + 1) queue[i](value);
    },
  };
}

const input = makeInput();
const rl = readline.createInterface({ input });
rl.on('line', (line) => console.log('line', JSON.stringify(line)));
rl.on('close', () => console.log('close'));

// '中' (e4 b8 ad) 被劈到两块；\r 与 \n 也被劈开，须合并为单一行界。
const bytes = Buffer.from('a中b\r\nc\n尾', 'utf8');
input.emit('data', bytes.subarray(0, 2));
input.emit('data', bytes.subarray(2, 6));
input.emit('data', bytes.subarray(6, 7));
input.emit('data', bytes.subarray(7));
rl.write('injected\nkept');
input.emit('end');
