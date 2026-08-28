const channels = new Map();

export class Channel {
  constructor(name) {
    this.name = name;
    this._subscribers = [];
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
  let existing = channels.get(name);
  if (existing === undefined) {
    existing = new Channel(name);
    channels.set(name, existing);
  }
  return existing;
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
