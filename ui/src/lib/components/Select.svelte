<script lang="ts" generics="T extends string | number">
  // A drop-down list in the app's style: the popup of a native <select> cannot be themed.
  // Focus stays on the button while the list is open (aria-activedescendant).
  import { followAnchor, portal, type PopupPlace } from '../popup';
  import Icon from './Icon.svelte';

  interface Props {
    value: T;
    options: { value: T; label: string }[];
    onchange?: (value: T) => void;
    /** Accessible name, when no <label> points at the control. */
    label?: string;
    width?: number;
    disabled?: boolean;
  }
  let { value = $bindable(), options, onchange, label, width, disabled = false }: Props = $props();

  const uid = $props.id();
  let open = $state(false);
  let active = $state(-1);
  let button = $state<HTMLButtonElement>();
  let list = $state<HTMLDivElement>();
  let place = $state<PopupPlace>({ left: 0, top: 0, width: 0, maxHeight: 280 });

  const selected = $derived(options.findIndex((o) => o.value === value));

  function show() {
    if (disabled) return;
    active = Math.max(0, selected);
    open = true;
  }

  function choose(index: number) {
    open = false;
    const option = options[index];
    if (!option || option.value === value) return;
    value = option.value;
    onchange?.(option.value);
  }

  $effect(() => {
    if (!open || !button || !list) return;
    return followAnchor(button, list, (p) => (place = p));
  });

  // Keep the highlighted option in view.
  $effect(() => {
    if (open && list && active >= 0) list.children[active]?.scrollIntoView({ block: 'nearest' });
  });

  let typed = '';
  let typedAt = 0;

  /** Jumps to the next option starting with the letters just typed. */
  function typeahead(char: string) {
    const now = Date.now();
    typed = now - typedAt > 700 ? char : typed + char;
    typedAt = now;
    const query = typed.toLowerCase();
    const from = typed.length === 1 ? active + 1 : active;
    for (let k = 0; k < options.length; k++) {
      const i = (from + k + options.length) % options.length;
      if (options[i].label.toLowerCase().startsWith(query)) {
        active = i;
        return;
      }
    }
  }

  function onkeydown(e: KeyboardEvent) {
    if (!open) {
      if (e.key === 'ArrowDown' || e.key === 'ArrowUp' || e.key === 'Enter' || e.key === ' ') {
        e.preventDefault();
        show();
      }
      return;
    }
    const last = options.length - 1;
    switch (e.key) {
      case 'ArrowDown':
        active = Math.min(last, active + 1);
        break;
      case 'ArrowUp':
        active = Math.max(0, active - 1);
        break;
      case 'PageDown':
        active = Math.min(last, active + 8);
        break;
      case 'PageUp':
        active = Math.max(0, active - 8);
        break;
      case 'Home':
        active = 0;
        break;
      case 'End':
        active = last;
        break;
      case 'Enter':
      case ' ':
        choose(active);
        break;
      case 'Escape':
        // Close the list, not the dialog around it.
        e.stopPropagation();
        open = false;
        break;
      case 'Tab':
        open = false;
        return;
      default:
        if (e.key.length !== 1 || e.ctrlKey || e.metaKey || e.altKey) return;
        typeahead(e.key);
    }
    e.preventDefault();
  }

  function onpointerdown(e: PointerEvent) {
    const target = e.target as Node;
    if (open && !button?.contains(target) && !list?.contains(target)) open = false;
  }
</script>

<svelte:window {onpointerdown} onblur={() => (open = false)} />

<button
  bind:this={button}
  type="button"
  class="input select"
  class:open
  style:width={width ? `${width}px` : undefined}
  role="combobox"
  aria-haspopup="listbox"
  aria-expanded={open}
  aria-controls={open ? `${uid}-list` : undefined}
  aria-activedescendant={open && active >= 0 ? `${uid}-${active}` : undefined}
  aria-label={label}
  {disabled}
  onclick={() => (open ? (open = false) : show())}
  {onkeydown}
>
  <span class="value">{options[selected]?.label ?? ''}</span>
  <span class="chevron"><Icon name="chevronDown" size={14} /></span>
</button>

{#if open}
  <!-- Clicks in the list must not move focus away from the button. -->
  <div
    bind:this={list}
    use:portal
    id="{uid}-list"
    class="popup-list"
    role="listbox"
    tabindex="-1"
    style:left="{place.left}px"
    style:top="{place.top}px"
    style:min-width="{place.width}px"
    style:max-height="{place.maxHeight}px"
    onmousedown={(e) => e.preventDefault()}
  >
    {#each options as option, i (option.value)}
      <div
        id="{uid}-{i}"
        class="popup-option"
        class:active={i === active}
        role="option"
        tabindex="-1"
        aria-selected={i === selected}
        onmousemove={() => (active = i)}
        onclick={() => choose(i)}
        onkeydown={() => {}}
      >
        <span class="text">{option.label}</span>
        <span class="tick">{#if i === selected}<Icon name="check" size={14} />{/if}</span>
      </div>
    {/each}
  </div>
{/if}

<style>
  .select {
    display: inline-flex;
    align-items: center;
    gap: 8px;
    padding-right: 7px;
    text-align: left;
  }
  .select:disabled {
    opacity: 0.5;
  }
  .select.open {
    border-color: var(--text-3);
    box-shadow: 0 0 0 3px var(--focus);
  }
  .value {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    white-space: nowrap;
    text-overflow: ellipsis;
  }
  .chevron {
    display: grid;
    color: var(--text-2);
    transition: transform 0.14s var(--ease);
  }
  .open .chevron {
    transform: rotate(180deg);
  }
</style>
