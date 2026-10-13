<script lang="ts" module>
  /** Colors under the fields when the caller gives none. */
  export const PRESET_COLORS = ['#2b2d31', '#8250df', '#5865f2', '#00c389', '#f0b232', '#da373c', '#d8dbe0'];

  /** How many colors the user can keep: two rows. */
  const MAX_SAVED = 14;

  interface EyeDropperResult {
    sRGBHex: string;
  }
  type EyeDropperConstructor = new () => { open(): Promise<EyeDropperResult> };
</script>

<script lang="ts">
  // A color swatch that opens a picker in the app's style: the native <input type="color">
  // opens the system's dialog. A saturation/brightness square and a hue strip, hex, RGB and HSL
  // fields, ready colors and the colors the user saved. Changes are applied as they are made.
  import { untrack } from 'svelte';
  import { hexToRgb, hslToHsv, hsvToHsl, hsvToRgb, parseHex, rgbToHex, rgbToHsv, type Hsv, type Rgb } from '../color';
  import { t } from '../i18n.svelte';
  import { followAnchor, portal, type PopupPlace } from '../popup';
  import Icon from './Icon.svelte';

  interface Props {
    /** `#rrggbb`. */
    value: string;
    onchange?: (value: string) => void;
    /** The color's name: the picker's title and the swatch's accessible name. */
    label?: string;
    disabled?: boolean;
    /** A color to go back to; a button for it shows while the value is another. */
    defaultValue?: string;
    presets?: string[];
    /** The user's saved colors; without `onsaved` the row is not shown. */
    saved?: string[];
    onsaved?: (saved: string[]) => void;
  }
  let {
    value,
    onchange,
    label = '',
    disabled = false,
    defaultValue,
    presets = PRESET_COLORS,
    saved = [],
    onsaved,
  }: Props = $props();

  const EyeDropper = (window as unknown as { EyeDropper?: EyeDropperConstructor }).EyeDropper;

  let open = $state(false);
  let button = $state<HTMLButtonElement>();
  let panel = $state<HTMLDivElement>();
  let area = $state<HTMLDivElement>();
  let hueStrip = $state<HTMLDivElement>();
  let place = $state<PopupPlace>({ left: 0, top: 0, width: 0, maxHeight: 520 });

  /** HSV keeps the hue and saturation of grays, which RGB loses. */
  let hsv = $state<Hsv>(rgbToHsv(hexToRgb(untrack(() => value))));
  const rgb = $derived(hsvToRgb(hsv));
  const hex = $derived(rgbToHex(rgb));
  const hsl = $derived(hsvToHsl(hsv));
  const isSaved = $derived(saved.includes(hex));

  // A new value from outside (another swatch's preset, settings reloaded).
  $effect(() => {
    const next = parseHex(value) ?? '#000000';
    if (next !== untrack(() => hex)) hsv = keepHue(rgbToHsv(hexToRgb(next)));
  });

  $effect(() => {
    if (!open || !button || !panel) return;
    return followAnchor(button, panel, (p) => (place = p), { align: 'end', maxHeight: 520 });
  });

  $effect(() => {
    if (open && area) area.focus();
  });

  /** A color from RGB keeps the current hue (and saturation) where it has none. */
  function keepHue(next: Hsv): Hsv {
    const gray = next.s === 0 || next.v === 0;
    return { h: gray ? hsv.h : next.h, s: next.v === 0 ? hsv.s : next.s, v: next.v };
  }

  function set(next: Hsv) {
    hsv = next;
    if (hex !== value) onchange?.(hex);
  }

  const setRgb = (next: Rgb) => set(keepHue(rgbToHsv(next)));
  const setHex = (next: string) => setRgb(hexToRgb(next));
  const clamp = (n: number, min: number, max: number) => Math.min(max, Math.max(min, n));

  function close(focusButton = true) {
    open = false;
    if (focusButton) button?.focus();
  }

  // ---- the square and the strip: drag, click, arrows ----------------------------------

  /** `inset`: px at each end the thumb's center does not go into (the strip's rounded ends). */
  function drag(e: PointerEvent, el: HTMLElement, move: (x: number, y: number) => void, inset = 0) {
    if (e.button !== 0) return;
    e.preventDefault();
    el.focus();
    el.setPointerCapture(e.pointerId);
    const at = (ev: PointerEvent) => {
      const r = el.getBoundingClientRect();
      const x = (ev.clientX - r.left - inset) / (r.width - 2 * inset);
      move(clamp(x, 0, 1), clamp((ev.clientY - r.top) / r.height, 0, 1));
    };
    at(e);
    const stop = () => {
      el.removeEventListener('pointermove', at);
      el.removeEventListener('pointerup', stop);
      el.removeEventListener('pointercancel', stop);
    };
    el.addEventListener('pointermove', at);
    el.addEventListener('pointerup', stop);
    el.addEventListener('pointercancel', stop);
  }

  function areaKey(e: KeyboardEvent) {
    const step = e.shiftKey ? 10 : 1;
    const moves: Record<string, [number, number]> = {
      ArrowLeft: [-step, 0],
      ArrowRight: [step, 0],
      ArrowUp: [0, step],
      ArrowDown: [0, -step],
    };
    const move = moves[e.key];
    if (!move) return;
    e.preventDefault();
    set({ ...hsv, s: clamp(Math.round(hsv.s) + move[0], 0, 100), v: clamp(Math.round(hsv.v) + move[1], 0, 100) });
  }

  function hueKey(e: KeyboardEvent) {
    const step = e.shiftKey ? 10 : 1;
    const h = Math.round(hsv.h);
    const next: Record<string, number> = {
      ArrowLeft: h - step,
      ArrowDown: h - step,
      ArrowRight: h + step,
      ArrowUp: h + step,
      Home: 0,
      End: 360,
    };
    if (!(e.key in next)) return;
    e.preventDefault();
    set({ ...hsv, h: clamp(next[e.key], 0, 360) });
  }

  // ---- fields ------------------------------------------------------------------------

  function hexInput(e: Event & { currentTarget: HTMLInputElement }) {
    // Applied while typing once all six digits are there; three on Enter or leaving.
    const text = e.currentTarget.value.trim().replace(/^#/, '');
    if (text.length === 6) {
      const next = parseHex(text);
      if (next) setHex(next);
    }
  }

  function hexCommit(input: HTMLInputElement) {
    const next = parseHex(input.value);
    if (next) setHex(next);
    input.value = hex.slice(1);
  }

  /** Arrow keys step a number field; Shift by ten. */
  function stepKey(e: KeyboardEvent, current: number, max: number, apply: (n: number) => void) {
    if (e.key !== 'ArrowUp' && e.key !== 'ArrowDown') return;
    e.preventDefault();
    const step = (e.key === 'ArrowUp' ? 1 : -1) * (e.shiftKey ? 10 : 1);
    apply(clamp(current + step, 0, max));
  }

  // ---- saved colors, eyedropper --------------------------------------------------------

  function save() {
    if (!onsaved || isSaved) return;
    onsaved([hex, ...saved].slice(0, MAX_SAVED));
  }

  function unsave() {
    onsaved?.(saved.filter((c) => c !== hex));
  }

  async function pick() {
    if (!EyeDropper) return;
    try {
      const { sRGBHex } = await new EyeDropper().open();
      const next = parseHex(sRGBHex);
      if (next) setHex(next);
    } catch {
      // Esc: nothing picked.
    }
    panel?.querySelector<HTMLElement>('.pick')?.focus();
  }

  // ---- keyboard and clicks outside --------------------------------------------------------

  function panelKey(e: KeyboardEvent) {
    if (e.key === 'Escape') {
      // Close the picker, not the dialog around it.
      e.preventDefault();
      e.stopPropagation();
      close();
    } else if (e.key === 'Tab' && panel) {
      // Keep focus inside the picker.
      const items = [...panel.querySelectorAll<HTMLElement>('input, button, [tabindex="0"]')].filter(
        (el) => !el.hasAttribute('disabled'),
      );
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

  function onpointerdown(e: PointerEvent) {
    const target = e.target as Node;
    if (open && !button?.contains(target) && !panel?.contains(target)) close(false);
  }
</script>

<svelte:window {onpointerdown} />

{#snippet field(letter: string, current: number, max: number, apply: (n: number) => void)}
  <label class="channel">
    <span>{letter}</span>
    <input
      class="mono"
      inputmode="numeric"
      maxlength="3"
      spellcheck="false"
      value={Math.round(current)}
      oninput={(e) => {
        const n = parseInt(e.currentTarget.value, 10);
        if (Number.isFinite(n)) apply(clamp(n, 0, max));
      }}
      onblur={(e) => (e.currentTarget.value = String(Math.round(current)))}
      onkeydown={(e) => stepKey(e, Math.round(current), max, apply)}
    />
  </label>
{/snippet}

{#snippet swatch(color: string)}
  <button
    type="button"
    class="tile"
    class:on={color === hex}
    style:--c={color}
    title={color}
    aria-label={color}
    aria-pressed={color === hex}
    onclick={() => setHex(color)}
  ></button>
{/snippet}

<button
  bind:this={button}
  type="button"
  class="swatch"
  class:open
  style:--c={hex}
  aria-label={label ? `${label}: ${hex}` : hex}
  aria-haspopup="dialog"
  aria-expanded={open}
  title={hex}
  {disabled}
  onclick={() => (open ? close() : (open = true))}
></button>

{#if open}
  <div
    bind:this={panel}
    use:portal
    class="picker"
    role="dialog"
    aria-label={label || hex}
    tabindex="-1"
    style:left="{place.left}px"
    style:top="{place.top}px"
    style:max-height="{place.maxHeight}px"
    onkeydown={panelKey}
  >
    <div class="pane">
      <div
        bind:this={area}
        class="area"
        style:--hue={`hsl(${hsv.h} 100% 50%)`}
        role="slider"
        tabindex="0"
        aria-label={t('color.area')}
        aria-valuemin={0}
        aria-valuemax={100}
        aria-valuenow={Math.round(hsv.s)}
        aria-valuetext="S {Math.round(hsv.s)}%, V {Math.round(hsv.v)}%"
        onpointerdown={(e) => area && drag(e, area, (x, y) => set({ ...hsv, s: x * 100, v: (1 - y) * 100 }))}
        onkeydown={areaKey}
      >
        <span class="thumb" style:left="{hsv.s}%" style:top="{100 - hsv.v}%" style:background={hex}></span>
      </div>
      <div
        bind:this={hueStrip}
        class="hue"
        role="slider"
        tabindex="0"
        aria-label={t('color.hue')}
        aria-valuemin={0}
        aria-valuemax={360}
        aria-valuenow={Math.round(hsv.h)}
        onpointerdown={(e) => hueStrip && drag(e, hueStrip, (x) => set({ ...hsv, h: x * 360 }), 7)}
        onkeydown={hueKey}
      >
        <span class="thumb" style:left="calc(7px + (100% - 14px) * {hsv.h / 360})" style:background={`hsl(${hsv.h} 100% 50%)`}></span>
      </div>
    </div>

    <div class="side">
      <div class="head">
        <span class="chip" style:--c={hex}></span>
        <span class="name">{label || hex}</span>
        {#if onsaved && isSaved}
          <button type="button" class="icon-btn small" title={t('color.unsave')} aria-label={t('color.unsave')} onclick={unsave}>
            <Icon name="trash" size={14} />
          </button>
        {/if}
        <button type="button" class="icon-btn small" title={t('color.close')} aria-label={t('color.close')} onclick={() => close()}>
          <Icon name="x" size={15} />
        </button>
      </div>

      <label class="hex">
        <span class="label">{t('color.hex')}</span>
        <span class="hex-row">
          <input
            class="input mono"
            maxlength="7"
            spellcheck="false"
            value={hex.slice(1)}
            oninput={hexInput}
            onblur={(e) => hexCommit(e.currentTarget)}
            onkeydown={(e) => e.key === 'Enter' && hexCommit(e.currentTarget)}
          />
          {#if EyeDropper}
            <button type="button" class="icon-btn pick" title={t('color.pick')} aria-label={t('color.pick')} onclick={pick}>
              <Icon name="pipette" size={15} />
            </button>
          {/if}
          {#if defaultValue && parseHex(defaultValue) !== hex}
            <button
              type="button"
              class="icon-btn"
              title={t('color.reset')}
              aria-label={t('color.reset')}
              onclick={() => setHex(defaultValue)}
            >
              <Icon name="refresh" size={15} />
            </button>
          {/if}
        </span>
      </label>

      <div class="fields">
        {@render field('R', rgb.r, 255, (r) => setRgb({ ...rgb, r }))}
        {@render field('G', rgb.g, 255, (g) => setRgb({ ...rgb, g }))}
        {@render field('B', rgb.b, 255, (b) => setRgb({ ...rgb, b }))}
        {@render field('H', hsl.h, 360, (h) => set(hslToHsv({ ...hsl, h })))}
        {@render field('S', hsl.s, 100, (s) => set(hslToHsv({ ...hsl, s })))}
        {@render field('L', hsl.l, 100, (l) => set(hslToHsv({ ...hsl, l })))}
      </div>

      <div class="tiles" role="group" aria-label={t('color.presets')}>
        {#each presets as color (color)}
          {@render swatch(color)}
        {/each}
      </div>

      {#if onsaved}
        <div class="tiles" role="group" aria-label={t('color.saved')}>
          {#each saved as color (color)}
            {@render swatch(color)}
          {/each}
          {#if !isSaved}
            <button type="button" class="tile add" title={t('color.save')} aria-label={t('color.save')} onclick={save}>
              <Icon name="plus" size={14} />
            </button>
          {/if}
        </div>
      {/if}
    </div>
  </div>
{/if}

<style>
  .swatch {
    flex: none;
    width: 56px;
    height: 26px;
    padding: 0;
    border: 1px solid var(--border);
    border-radius: 8px;
    background: var(--c);
    box-shadow: inset 0 0 0 2px var(--bg);
    transition: box-shadow 0.12s var(--ease);
  }
  .swatch:disabled {
    pointer-events: none;
  }
  .swatch.open {
    box-shadow:
      inset 0 0 0 2px var(--bg),
      0 0 0 3px var(--focus);
  }

  .picker {
    position: fixed;
    z-index: 70;
    display: flex;
    gap: 18px;
    width: 500px;
    max-width: calc(100vw - 16px);
    overflow-y: auto;
    padding: 16px;
    border-radius: 14px;
    border: 1px solid var(--border);
    background: var(--elevated);
    box-shadow: var(--shadow);
    animation: popup-in 0.1s var(--ease);
  }
  @media (prefers-reduced-motion: reduce) {
    .picker {
      animation: none;
    }
  }

  .pane {
    display: flex;
    flex-direction: column;
    gap: 14px;
    flex: 1;
    min-width: 0;
  }
  .area {
    position: relative;
    height: 196px;
    border-radius: var(--radius-sm);
    background:
      linear-gradient(to top, #000, transparent),
      linear-gradient(to right, #fff, var(--hue));
    touch-action: none;
  }
  .hue {
    position: relative;
    height: 14px;
    border-radius: 7px;
    /* The hues run between the centers of the rounded ends, where the thumb's center goes. */
    background: linear-gradient(to right, #f00 7px, #ff0, #0f0, #0ff, #00f, #f0f, #f00 calc(100% - 7px));
    touch-action: none;
  }
  .thumb {
    position: absolute;
    width: 18px;
    height: 18px;
    border: 2.5px solid #fff;
    border-radius: 50%;
    box-shadow:
      0 0 0 1px rgba(0, 0, 0, 0.25),
      0 1px 4px rgba(0, 0, 0, 0.35);
    transform: translate(-50%, -50%);
    pointer-events: none;
  }
  .hue .thumb {
    top: 50%;
  }
  .area:focus-visible,
  .hue:focus-visible {
    outline: 2px solid var(--focus);
    outline-offset: 2px;
  }

  .side {
    display: flex;
    flex-direction: column;
    gap: 12px;
    width: 220px;
    flex: none;
  }
  .head {
    display: flex;
    align-items: center;
    gap: 10px;
    min-height: 28px;
  }
  .chip {
    flex: none;
    width: 26px;
    height: 26px;
    border-radius: var(--radius-xs);
    background: var(--c);
    box-shadow: inset 0 0 0 1px color-mix(in srgb, var(--text) 12%, transparent);
  }
  .name {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    white-space: nowrap;
    text-overflow: ellipsis;
    font-weight: 500;
  }
  .hex {
    display: flex;
    flex-direction: column;
    gap: 5px;
  }
  .hex-row {
    display: flex;
    align-items: center;
    gap: 4px;
  }
  .hex-row .input {
    flex: 1;
    min-width: 0;
  }
  .hex-row .icon-btn {
    flex: none;
    width: 30px;
    height: 30px;
  }
  .fields {
    display: grid;
    grid-template-columns: repeat(3, 1fr);
    gap: 6px;
  }
  .channel {
    display: flex;
    align-items: center;
    gap: 6px;
    height: 32px;
    padding: 0 8px;
    border-radius: var(--radius-sm);
    border: 1px solid var(--border);
    background: var(--input);
    transition:
      border-color 0.12s var(--ease),
      box-shadow 0.12s var(--ease);
  }
  .channel:focus-within {
    border-color: var(--text-3);
    box-shadow: 0 0 0 3px var(--focus);
  }
  .channel span {
    font-size: 11px;
    color: var(--text-3);
  }
  .channel input {
    width: 100%;
    min-width: 0;
    padding: 0;
    border: 0;
    background: none;
  }
  .tiles {
    display: grid;
    grid-template-columns: repeat(7, 1fr);
    gap: 6px;
  }
  .tile {
    aspect-ratio: 1;
    padding: 0;
    border: 0;
    border-radius: var(--radius-xs);
    background: var(--c);
    box-shadow: inset 0 0 0 1px color-mix(in srgb, var(--text) 12%, transparent);
    transition: transform 0.1s var(--ease);
  }
  .tile:hover {
    transform: scale(1.08);
  }
  .tile.on {
    box-shadow:
      inset 0 0 0 2px var(--elevated),
      0 0 0 2px var(--text-2);
  }
  .tile.add {
    display: grid;
    place-items: center;
    background: transparent;
    color: var(--text-2);
    box-shadow: none;
    border: 1px dashed var(--border);
  }
  .tile.add:hover {
    color: var(--text);
    border-color: var(--text-3);
    transform: none;
  }
</style>
