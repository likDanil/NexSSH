<script lang="ts">
  import type { Snippet } from 'svelte';
  import { onMount } from 'svelte';

  interface Props {
    title?: string;
    subtitle?: string;
    width?: number;
    onclose: () => void;
    children: Snippet;
    footer?: Snippet;
  }
  let { title, subtitle, width = 460, onclose, children, footer }: Props = $props();

  let dialog: HTMLDivElement;

  onMount(() => {
    const previous = document.activeElement as HTMLElement | null;
    // Focus the first field (or the dialog itself) so Esc and Enter work right away.
    const first = dialog.querySelector<HTMLElement>('[autofocus], input:not([type=hidden]), select, textarea, button');
    (first ?? dialog).focus();
    return () => previous?.focus?.();
  });

  function onkeydown(e: KeyboardEvent) {
    if (e.key === 'Escape') {
      e.preventDefault();
      e.stopPropagation();
      onclose();
    } else if (e.key === 'Tab') {
      // Keep focus inside the dialog.
      const items = [
        ...dialog.querySelectorAll<HTMLElement>('input, select, textarea, button, [tabindex]:not([tabindex="-1"])'),
      ].filter((el) => !el.hasAttribute('disabled') && el.offsetParent !== null);
      if (!items.length) return;
      const first = items[0];
      const last = items[items.length - 1];
      if (e.shiftKey && document.activeElement === first) {
        e.preventDefault();
        last.focus();
      } else if (!e.shiftKey && document.activeElement === last) {
        e.preventDefault();
        first.focus();
      }
    }
  }
</script>

<!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
<div class="backdrop" onclick={(e) => e.target === e.currentTarget && onclose()}>
  <div
    class="dialog"
    role="dialog"
    aria-modal="true"
    aria-label={title}
    tabindex="-1"
    style:width="{width}px"
    bind:this={dialog}
    {onkeydown}
  >
    {#if title}
      <header>
        <h2>{title}</h2>
        {#if subtitle}<p>{subtitle}</p>{/if}
      </header>
    {/if}
    <div class="body">
      {@render children()}
    </div>
    {#if footer}
      <footer>
        {@render footer()}
      </footer>
    {/if}
  </div>
</div>

<style>
  .backdrop {
    position: fixed;
    inset: 0;
    z-index: 50;
    display: grid;
    place-items: start center;
    padding: 11vh 16px 16px;
    background: var(--overlay);
    animation: fade 0.12s var(--ease);
  }
  .dialog {
    max-width: 100%;
    max-height: 80vh;
    display: flex;
    flex-direction: column;
    border-radius: 12px;
    border: 1px solid var(--border);
    background: var(--bg);
    box-shadow: var(--shadow);
    animation: rise 0.16s var(--ease);
    overflow: hidden;
  }
  header {
    padding: 18px 20px 4px;
  }
  h2 {
    margin: 0;
    font-size: 15px;
    font-weight: 600;
    letter-spacing: -0.01em;
  }
  header p {
    margin: 4px 0 0;
    color: var(--text-2);
  }
  .body {
    padding: 14px 20px 18px;
    overflow: auto;
  }
  footer {
    display: flex;
    justify-content: flex-end;
    align-items: center;
    gap: 8px;
    padding: 12px 20px;
    border-top: 1px solid var(--border-soft);
    background: var(--frame);
  }
  @keyframes fade {
    from {
      opacity: 0;
    }
  }
  @keyframes rise {
    from {
      opacity: 0;
      transform: translateY(6px) scale(0.99);
    }
  }
</style>
