<script lang="ts">
  let {
    createSource = defaultSource,
  }: { createSource?: (url: string) => EventSource | null } = $props();

  let bands = $state<number[]>([]);
  const bars = $derived(bands.length > 0 ? bands : new Array(64).fill(0));

  function defaultSource(url: string): EventSource | null {
    if (typeof EventSource === 'undefined') return null;
    return new EventSource(url);
  }

  $effect(() => {
    const source = createSource('/api/visualizer/events');
    if (!source) return;
    const handler = (event: MessageEvent) => {
      try {
        const frame = JSON.parse(String(event.data)) as { bands?: number[] };
        if (Array.isArray(frame.bands)) bands = frame.bands;
      } catch {
        // Ignore a malformed frame.
      }
    };
    source.addEventListener('visualizer', handler as EventListener);
    return () => {
      source.removeEventListener('visualizer', handler as EventListener);
      source.close();
    };
  });
</script>

<div class="visualizer" aria-hidden="true">
  {#each bars as value}
    <span class="bar" style={`height: ${Math.min(100, Math.max(2, value))}%`}></span>
  {/each}
</div>

<style>
  .visualizer {
    display: flex;
    align-items: flex-end;
    gap: 2px;
    width: 100%;
    height: 72px;
    padding: 0.4rem 0;
    overflow: hidden;
    border-top: 1px solid var(--border);
    border-bottom: 1px solid var(--border);
  }

  .bar {
    flex: 1;
    min-height: 2px;
    background: linear-gradient(to top, var(--accent), var(--accent-2));
    opacity: 0.85;
  }
</style>
