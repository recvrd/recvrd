<script lang="ts">
  import type { MenuItem } from '../types';
  import { clampMenuPosition, type MenuPosition } from '../menu-position';

  let {
    x,
    y,
    items,
    onclose,
  }: {
    x: number;
    y: number;
    items: MenuItem[];
    onclose: () => void;
  } = $props();

  let menu: HTMLDivElement | undefined = $state();
  let position: MenuPosition | null = $state(null);

  $effect(() => {
    if (menu) {
      const rect = menu.getBoundingClientRect();
      position = clampMenuPosition(
        x,
        y,
        rect.width,
        rect.height,
        window.innerWidth,
        window.innerHeight,
      );
    }
  });

  $effect(() => {
    const close = (event: Event) => {
      if (menu && event.target instanceof Node && menu.contains(event.target)) return;
      onclose();
    };
    const key = (event: KeyboardEvent) => {
      if (event.key === 'Escape') onclose();
    };
    window.addEventListener('pointerdown', close);
    window.addEventListener('keydown', key);
    return () => {
      window.removeEventListener('pointerdown', close);
      window.removeEventListener('keydown', key);
    };
  });
</script>

<div
  class="menu"
  role="menu"
  bind:this={menu}
  style:left={`${position?.left ?? x}px`}
  style:top={`${position?.top ?? y}px`}
>
  {#each items as item}
    <button
      role="menuitem"
      disabled={item.disabled}
      onclick={() => {
        item.action();
        onclose();
      }}
    >
      {item.label}
    </button>
  {/each}
</div>

<style>
  .menu {
    position: fixed;
    z-index: 60;
    min-width: 10rem;
    max-height: calc(100vh - 1rem);
    overflow-y: auto;
    display: flex;
    flex-direction: column;
    padding: 0.3rem;
    border: 1px dashed var(--border);
    border-radius: 4px;
    background: color-mix(in srgb, var(--bg-raised) 80%, transparent);
    backdrop-filter: blur(6px);
    box-shadow:
      0 16px 48px rgb(3 8 18 / 60%),
      0 0 18px color-mix(in srgb, var(--accent) 14%, transparent);
  }

  button {
    border: none;
    background: transparent;
    text-align: left;
    padding: 0.45rem 0.6rem;
    border-radius: 5px;
    font-size: 0.84rem;
  }

  button:hover:not(:disabled) {
    background: var(--selection);
  }
</style>
