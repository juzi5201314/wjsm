const channels = new Map();

// 对齐 Node determineSpecificType：错误消息中描述收到的实参。
function describeReceived(value) {
  if (value === null) return 'null';
  if (value === undefined) return 'undefined';
  const type = typeof value;
  if (type === 'function') return 'function ' + value.name;
  if (type === 'object') {
    const ctor = value.constructor;
    if (ctor !== undefined && ctor !== null) {
      return 'an instance of ' + ctor.name;
    }
    // 无构造器（如 Object.create(null)）：对齐 util.inspect(value, { depth: -1 }) 的折叠渲染。
    return Object.keys(value).length === 0
      ? '[Object: null prototype] {}'
      : '[Object: null prototype]';
  }
  let inspected;
  if (type === 'bigint') {
    inspected = String(value) + 'n';
  } else if (type === 'number' && Object.is(value, -0)) {
    inspected = '-0';
  } else {
    inspected = String(value);
  }
  return 'type ' + type + ' (' + inspected + ')';
}

function invalidChannelName(name) {
  const err = new TypeError(
    'The "channel" argument must be one of type string or symbol. Received ' +
      describeReceived(name)
  );
  err.code = 'ERR_INVALID_ARG_TYPE';
  return err;
}

export class Channel {
  constructor(name) {
    this.name = name;
    this._subscribers = [];
    // 对齐 Node：直接构造不校验 name，但登记（覆盖）到 channels 表，
    // 之后 channel(name) 返回同一实例。
    channels.set(name, this);
  }

  get hasSubscribers() {
    return this._subscribers.length > 0;
  }

  subscribe(onMessage) {
    if (typeof onMessage !== 'function') {
      throw new TypeError('The "onMessage" argument must be of type function');
    }
    this._subscribers.push(onMessage);
  }

  unsubscribe(onMessage) {
    const index = this._subscribers.indexOf(onMessage);
    if (index < 0) return false;
    this._subscribers.splice(index, 1);
    return true;
  }

  publish(message) {
    // 复制订阅者快照：发布期间的订阅变更不影响本次分发。
    const subscribers = this._subscribers.slice();
    for (let i = 0; i < subscribers.length; i = i + 1) {
      subscribers[i](message, this.name);
    }
  }
}

export function channel(name) {
  // 对齐 Node：先查表后校验——直接 new Channel 登记过的任意 name 可查回，
  // 未登记的非 string/symbol 名称抛 ERR_INVALID_ARG_TYPE。
  const existing = channels.get(name);
  if (existing !== undefined) return existing;
  if (typeof name !== 'string' && typeof name !== 'symbol') {
    throw invalidChannelName(name);
  }
  return new Channel(name);
}

export function subscribe(name, onMessage) {
  channel(name).subscribe(onMessage);
}

export function unsubscribe(name, onMessage) {
  return channel(name).unsubscribe(onMessage);
}

export function hasSubscribers(name) {
  const existing = channels.get(name);
  return existing !== undefined && existing.hasSubscribers;
}

export default { channel, subscribe, unsubscribe, hasSubscribers, Channel };
