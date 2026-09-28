<script module lang="ts">
  export interface Suggestion {
    value: string;
    /** Shown dimmed on the right, e.g. the key type or the host of a server. */
    hint?: string;
  }
</script>

<script lang="ts">
  // A text field with suggestions in the app's style (the popup of a native <datalist>
  // cannot be themed). Suggestions only help: any text can be typed.
  import { followAnchor, portal, type PopupPlace } from '../popup';

  interface Props {
    value: string;
    suggestions: Suggestion[];
    id?: string;
    placeholder?: string;
    mono?: boolean;
    /** Accessible name, when no <label> points at the field. */
    label?: string;
  }
  let { value = $bindable(), suggestions, id, placeholder, mono = false, label }: Props = $props();

  const uid = $props.id();
  let open = $state(false);
  /** After typing, suggestions are filtered by the text; when just opened, all are shown. */
  let filtering = $state(false);
  let active = $state(-1);
  let input = $state<HTMLInputElement>();
  let list = $state<HTMLDivElement>();
  let place = $state<PopupPlace>({ left: 0, top: 0, width: 0, maxHeight: 280 });

  const shown = $derived.by(() => {
    const query = value.trim().toLowerCase();
    if (!filtering || !query) return suggestions;
    return suggestions.filter(
      (s) => s.value.toLowerCase().includes(query) || !!s.hint?.toLowerCase().includes(query),
    );
  });
  const visible = $derived(open && shown.length > 0);

  function show(filter: boolean) {
    filtering = filter;
    active = -1;
    open = true;
  }

  function pick(s: Suggestion) {
    value = s.value;
    open = false;
  }

  $effect(() => {
    if (!visible || !input || !list) return;
    return followAnchor(input, list, (p) => (place = p));
  });

  $effect(() => {
    if (visible && list && active >= 0) list.children[active]?.scrollIntoView({ block: 'nearest' });
  });

  function onkeydown(e: KeyboardEvent) {
    switch (e.key) {
      case 'ArrowDown':
        if (!visible) show(false);
        else active = Math.min(shown.length - 1, active + 1);
        break;
      case 'ArrowUp':
        if (!visible) return;
        active = Math.max(0, active - 1);
        break;
      case 'Enter':
        // Enter picks the highlighted suggestion; otherwise it submits the form as usual.
        if (!visible || active < 0) return;
        pick(shown[active]);
        break;
      case 'Escape':
        if (!visible) return;
        e.stopPropagation();
        open = false;
        break;
      default:
        return;
    }
    e.preventDefault();
  }
</script>

<input
  bind:this={input}
  bind:value
  {id}
  class="input"
  class:mono
  {placeholder}
  spellcheck="false"
  autocomplete="off"
  role="combobox"
  aria-autocomplete="list"
  aria-expanded={visible}
  aria-controls={visible ? `${uid}-list` : undefined}
  aria-activedescendant={visible && active >= 0 ? `${uid}-${active}` : undefined}
  aria-label={label}
  onmousedown={() => !open && show(false)}
  oninput={() => show(true)}
  onblur={() => (open = false)}
  {onkeydown}
/>

{#if visible}
  <!-- Clicks in the list must not take focus from the field. -->
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
    <!-- Keyed by position: two servers may share a name. -->
    {#each shown as s, i (i)}
      <div
        id="{uid}-{i}"
        class="popup-option"
        class:active={i === active}
        class:mono
        role="option"
        tabindex="-1"
        aria-selected={s.value === value}
        onmousemove={() => (active = i)}
        onclick={() => pick(s)}
        onkeydown={() => {}}
      >
        <span class="text">{s.value}</span>
        {#if s.hint}<span class="hint">{s.hint}</span>{/if}
      </div>
    {/each}
  </div>
{/if}
