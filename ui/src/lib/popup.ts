// Lists and panels that open under a control (Select, Suggest, ColorPicker). They are rendered at the end of
// <body>, so a dialog's `overflow: hidden` or open animation cannot clip or shift them.

/** Svelte action: moves the element to the end of <body>. */
export function portal(node: HTMLElement) {
  document.body.appendChild(node);
  return {
    destroy() {
      node.remove();
    },
  };
}

export interface PopupPlace {
  left: number;
  top: number;
  /** The anchor's width: the list is at least as wide. */
  width: number;
  maxHeight: number;
}

const GAP = 4;
const MARGIN = 8;
const MAX_HEIGHT = 280;

export interface PopupOptions {
  /** Line the popup up with the anchor's left edge (`start`) or its right edge (`end`). */
  align?: 'start' | 'end';
  maxHeight?: number;
}

/** Under the anchor, or above it when the list fits better there; always inside the window. */
export function placePopup(
  anchor: HTMLElement,
  popup: HTMLElement,
  { align = 'start', maxHeight: most = MAX_HEIGHT }: PopupOptions = {},
): PopupPlace {
  const r = anchor.getBoundingClientRect();
  const below = window.innerHeight - r.bottom - GAP - MARGIN;
  const above = r.top - GAP - MARGIN;
  const wanted = Math.min(popup.scrollHeight, most);
  const up = wanted > below && above > below;
  const maxHeight = Math.max(80, Math.min(most, up ? above : below));
  const width = Math.max(r.width, popup.offsetWidth);
  const left = align === 'end' ? r.right - width : r.left;
  return {
    left: Math.max(MARGIN, Math.min(left, window.innerWidth - width - MARGIN)),
    top: up ? r.top - GAP - Math.min(wanted, maxHeight) : r.bottom + GAP,
    width: r.width,
    maxHeight,
  };
}

/** Keeps a list placed while it is open (scrolling, resizing); returns the cleanup. */
export function followAnchor(
  anchor: HTMLElement,
  popup: HTMLElement,
  place: (p: PopupPlace) => void,
  options?: PopupOptions,
) {
  const update = () => place(placePopup(anchor, popup, options));
  const onScroll = (e: Event) => {
    if (e.target !== popup) update();
  };
  update();
  window.addEventListener('scroll', onScroll, true);
  window.addEventListener('resize', update);
  return () => {
    window.removeEventListener('scroll', onScroll, true);
    window.removeEventListener('resize', update);
  };
}
