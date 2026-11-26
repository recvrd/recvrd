import { render } from '@testing-library/svelte';
import { tick } from 'svelte';
import { describe, expect, it } from 'vitest';
import Visualizer from './Visualizer.svelte';

class FakeSource {
  listener: ((event: MessageEvent) => void) | null = null;

  addEventListener(_type: string, listener: EventListener): void {
    this.listener = listener as (event: MessageEvent) => void;
  }

  removeEventListener(): void {
    this.listener = null;
  }

  close(): void {}

  emit(bands: number[]): void {
    this.listener?.({ data: JSON.stringify({ bands, peak: 128 }) } as MessageEvent);
  }
}

describe('Visualizer', () => {
  it('renders the frame bands', async () => {
    const source = new FakeSource();
    const { container } = render(Visualizer, {
      props: { createSource: () => source as unknown as EventSource },
    });
    expect(container.querySelectorAll('.bar')).toHaveLength(64);

    const bands = new Array(64).fill(0);
    bands[1] = 128;
    bands[2] = 255;
    source.emit(bands);
    await tick();

    const bars = container.querySelectorAll<HTMLElement>('.bar');
    expect(bars[0].style.height).toBe('2%');
    expect(bars[1].style.height).toBe('100%');
    expect(bars[2].style.height).toBe('100%');
    expect(bars[3].style.height).toBe('2%');
  });
});
