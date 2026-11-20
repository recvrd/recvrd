import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { EventStream } from './sse';
import { FakeEventSource } from '../test/fixtures';

function requireGlobalReceiver(this: unknown): void {
  if (this !== globalThis) throw new TypeError('Illegal invocation');
}

describe('EventStream', () => {
  beforeEach(() => {
    vi.useFakeTimers();
  });

  afterEach(() => {
    vi.unstubAllGlobals();
    vi.useRealTimers();
  });

  it('connects, reports state and dispatches named and unnamed events', () => {
    const sources: FakeEventSource[] = [];
    const events: unknown[] = [];
    const states: string[] = [];
    const stream = new EventStream({
      create: (url) => {
        const source = new FakeEventSource(url);
        sources.push(source);
        return source;
      },
      onEvent: (event) => events.push(event),
      onState: (state) => states.push(state),
    });

    stream.connect();
    expect(states).toEqual(['connecting']);
    expect(sources[0].url).toBe('/api/events');

    sources[0].open();
    expect(states).toEqual(['connecting', 'online']);

    sources[0].emitNamed('snapshot', { type: 'library_changed' });
    sources[0].emit({ type: 'track_ended', provider: 'youtube', track_id: 'v1' });
    expect(events).toHaveLength(2);

    stream.close();
    expect(sources[0].closed).toBe(true);
  });

  it('reports reconnect and offline states', () => {
    const sources: FakeEventSource[] = [];
    const states: string[] = [];
    const stream = new EventStream({
      create: (url) => {
        const source = new FakeEventSource(url);
        sources.push(source);
        return source;
      },
      onEvent: () => {},
      onState: (state) => states.push(state),
      offlineAfterMs: 2000,
    });

    stream.connect();
    sources[0].open();
    sources[0].fail();
    expect(states.at(-1)).toBe('reconnecting');

    vi.advanceTimersByTime(2000);
    expect(states.at(-1)).toBe('offline');

    sources.at(-1)!.open();
    expect(states.at(-1)).toBe('online');
    stream.close();
  });

  it('backs off exponentially and keeps the last event id', () => {
    const sources: FakeEventSource[] = [];
    const stream = new EventStream({
      create: (url) => {
        const source = new FakeEventSource(url);
        sources.push(source);
        return source;
      },
      onEvent: () => {},
      onState: () => {},
      retryBaseMs: 100,
      retryMaxMs: 400,
      // Full delay, so the exponential steps below are exact.
      randomFn: () => 1,
    });

    stream.connect();
    sources[0].emit({ type: 'library_changed' });
    sources[0].fail();
    vi.advanceTimersByTime(100);
    expect(sources).toHaveLength(2);
    expect(sources[1].url).toBe('/api/events?last_event_id=1');

    sources[1].fail();
    vi.advanceTimersByTime(199);
    expect(sources).toHaveLength(2);
    vi.advanceTimersByTime(1);
    expect(sources).toHaveLength(3);

    sources[2].fail();
    vi.advanceTimersByTime(400);
    expect(sources).toHaveLength(4);
    stream.close();
  });

  it('replaces the source on retry', () => {
    const sources: FakeEventSource[] = [];
    const stream = new EventStream({
      create: (url) => {
        const source = new FakeEventSource(url);
        sources.push(source);
        return source;
      },
      onEvent: () => {},
      onState: () => {},
    });

    stream.connect();
    stream.retry();
    expect(sources).toHaveLength(2);
    expect(sources[0].closed).toBe(true);
    expect(sources[1].closed).toBe(false);
    stream.close();
  });

  it('reports malformed events without throwing', () => {
    const errors: unknown[] = [];
    const sources: FakeEventSource[] = [];
    const stream = new EventStream({
      create: (url) => {
        const source = new FakeEventSource(url);
        sources.push(source);
        return source;
      },
      onEvent: () => {},
      onState: () => {},
      onParseError: (error) => errors.push(error),
    });
    stream.connect();
    sources[0].onmessage?.(new MessageEvent('message', { data: '{bad' }));
    expect(errors).toHaveLength(1);
    stream.close();
  });

  it('reconnects after silence and heartbeats reset the watchdog', () => {
    const sources: FakeEventSource[] = [];
    const stream = new EventStream({
      create: (url) => {
        const source = new FakeEventSource(url);
        sources.push(source);
        return source;
      },
      onEvent: () => {},
      onState: () => {},
      watchdogIntervalMs: 1000,
      silenceMs: 3000,
    });

    stream.connect();
    sources[0].open();
    vi.advanceTimersByTime(2000);
    expect(sources).toHaveLength(1);

    sources[0].emitNamed('heartbeat', {});
    vi.advanceTimersByTime(2000);
    expect(sources).toHaveLength(1);

    vi.advanceTimersByTime(1000);
    expect(sources).toHaveLength(2);
    expect(sources[0].closed).toBe(true);
    expect(sources[1].closed).toBe(false);
    stream.close();
  });

  it('heartbeat does not change the last event id', () => {
    const sources: FakeEventSource[] = [];
    const stream = new EventStream({
      create: (url) => {
        const source = new FakeEventSource(url);
        sources.push(source);
        return source;
      },
      onEvent: () => {},
      onState: () => {},
      retryBaseMs: 100,
      randomFn: () => 1,
    });

    stream.connect();
    sources[0].emit({ type: 'library_changed' });
    sources[0].emitNamed('heartbeat', {});
    sources[0].fail();
    vi.advanceTimersByTime(100);
    expect(sources).toHaveLength(2);
    expect(sources[1].url).toBe('/api/events?last_event_id=1');
    stream.close();
  });

  it('retry clears a pending retry and offline timer', () => {
    const sources: FakeEventSource[] = [];
    const states: string[] = [];
    const stream = new EventStream({
      create: (url) => {
        const source = new FakeEventSource(url);
        sources.push(source);
        return source;
      },
      onEvent: () => {},
      onState: (state) => states.push(state),
      retryBaseMs: 1000,
      offlineAfterMs: 2000,
      randomFn: () => 1,
    });

    stream.connect();
    sources[0].open();
    sources[0].fail();
    vi.advanceTimersByTime(500);
    stream.retry();
    expect(sources).toHaveLength(2);
    expect(sources[0].closed).toBe(true);

    // Past both the old retry delay (1000 ms after the failure) and the
    // old offline delay (2000 ms): neither timer may fire now.
    vi.advanceTimersByTime(2000);
    expect(sources).toHaveLength(2);
    expect(states).not.toContain('offline');
    stream.close();
  });

  it('jitters the backoff between half and the full delay', () => {
    const makeStream = (randomFn: () => number) => {
      const sources: FakeEventSource[] = [];
      const stream = new EventStream({
        create: (url) => {
          const source = new FakeEventSource(url);
          sources.push(source);
          return source;
        },
        onEvent: () => {},
        onState: () => {},
        retryBaseMs: 100,
        randomFn,
      });
      stream.connect();
      sources[0].fail();
      return { sources, stream };
    };

    const half = makeStream(() => 0);
    vi.advanceTimersByTime(49);
    expect(half.sources).toHaveLength(1);
    vi.advanceTimersByTime(1);
    expect(half.sources).toHaveLength(2);
    half.stream.close();

    const full = makeStream(() => 1);
    vi.advanceTimersByTime(99);
    expect(full.sources).toHaveLength(1);
    vi.advanceTimersByTime(1);
    expect(full.sources).toHaveLength(2);
    full.stream.close();
  });

  it('calls onReconnect on a later open, not the first', () => {
    const sources: FakeEventSource[] = [];
    let reconnects = 0;
    const stream = new EventStream({
      create: (url) => {
        const source = new FakeEventSource(url);
        sources.push(source);
        return source;
      },
      onEvent: () => {},
      onState: () => {},
      onReconnect: () => {
        reconnects += 1;
      },
      randomFn: () => 1,
    });

    stream.connect();
    sources[0].open();
    expect(reconnects).toBe(0);

    sources[0].fail();
    vi.advanceTimersByTime(1000);
    sources[1].open();
    expect(reconnects).toBe(1);
    stream.close();
  });

  it('reconnects on wake triggers when offline or stale, not when fresh', () => {
    Object.defineProperty(document, 'visibilityState', {
      value: 'visible',
      configurable: true,
    });
    const sources: FakeEventSource[] = [];
    const stream = new EventStream({
      create: (url) => {
        const source = new FakeEventSource(url);
        sources.push(source);
        return source;
      },
      onEvent: () => {},
      onState: () => {},
      // Watchdog parked far out so only the wake triggers reconnect here.
      watchdogIntervalMs: 60000,
      silenceMs: 3000,
      randomFn: () => 1,
    });

    stream.connect();
    sources[0].open();
    window.dispatchEvent(new Event('online'));
    document.dispatchEvent(new Event('visibilitychange'));
    expect(sources).toHaveLength(1);

    // Offline (in backoff after a failure): an online event reconnects now.
    sources[0].fail();
    window.dispatchEvent(new Event('online'));
    expect(sources).toHaveLength(2);
    sources[1].open();

    // Online but stale: a visibility change reconnects.
    vi.advanceTimersByTime(3000);
    document.dispatchEvent(new Event('visibilitychange'));
    expect(sources).toHaveLength(3);
    sources[2].open();

    // Online but stale again: pageshow reconnects.
    vi.advanceTimersByTime(3000);
    window.dispatchEvent(new Event('pageshow'));
    expect(sources).toHaveLength(4);

    stream.close();
    window.dispatchEvent(new Event('online'));
    expect(sources).toHaveLength(4);
  });

  it('uses the browser EventSource and timers by default', () => {
    const sources: FakeEventSource[] = [];
    class BrowserSource extends FakeEventSource {
      constructor(url: string) {
        super(url);
        sources.push(this);
      }
    }
    const timeout = globalThis.setTimeout;
    const clear = globalThis.clearTimeout;
    const interval = globalThis.setInterval;
    const clearIntervalFn = globalThis.clearInterval;
    vi.stubGlobal('EventSource', BrowserSource);
    vi.stubGlobal('setTimeout', function (this: unknown, handler: () => void, ms?: number) {
      requireGlobalReceiver.call(this);
      return timeout(handler, ms);
    });
    vi.stubGlobal('clearTimeout', function (this: unknown, handle: Parameters<typeof clearTimeout>[0]) {
      requireGlobalReceiver.call(this);
      clear(handle);
    });
    vi.stubGlobal('setInterval', function (this: unknown, handler: () => void, ms?: number) {
      requireGlobalReceiver.call(this);
      return interval(handler, ms);
    });
    vi.stubGlobal('clearInterval', function (this: unknown, handle: Parameters<typeof clearInterval>[0]) {
      requireGlobalReceiver.call(this);
      clearIntervalFn(handle);
    });

    const stream = new EventStream({
      onEvent: () => {},
      onState: () => {},
    });
    expect(() => {
      stream.connect();
      sources[0].fail();
      stream.close();
    }).not.toThrow();
    expect(sources).toHaveLength(1);
    expect(sources[0].url).toBe('/api/events');
    expect(sources[0].closed).toBe(true);
  });
});
