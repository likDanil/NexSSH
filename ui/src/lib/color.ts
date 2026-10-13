// Color math for the color picker (ColorPicker.svelte): `#rrggbb` and the RGB, HSV and HSL
// the picker shows. Tested in ui/tests/color.test.ts.

export interface Rgb {
  /** 0–255 each. */
  r: number;
  g: number;
  b: number;
}

export interface Hsv {
  /** 0–360. */
  h: number;
  /** 0–100 each. */
  s: number;
  v: number;
}

export interface Hsl {
  /** 0–360. */
  h: number;
  /** 0–100 each. */
  s: number;
  l: number;
}

const clamp = (n: number, min: number, max: number) => Math.min(max, Math.max(min, n));

/** `#rrggbb` from what the user typed: 3 or 6 hex digits, with or without `#`; else null. */
export function parseHex(text: string): string | null {
  const m = /^#?([0-9a-f]{3}|[0-9a-f]{6})$/i.exec(text.trim());
  if (!m) return null;
  const digits = m[1].length === 3 ? [...m[1]].map((d) => d + d).join('') : m[1];
  return `#${digits.toLowerCase()}`;
}

export function hexToRgb(hex: string): Rgb {
  const n = parseInt((parseHex(hex) ?? '#000000').slice(1), 16);
  return { r: n >> 16, g: (n >> 8) & 255, b: n & 255 };
}

export function rgbToHex({ r, g, b }: Rgb): string {
  const part = (n: number) => Math.round(clamp(n, 0, 255)).toString(16).padStart(2, '0');
  return `#${part(r)}${part(g)}${part(b)}`;
}

export function rgbToHsv({ r, g, b }: Rgb): Hsv {
  const [rr, gg, bb] = [r / 255, g / 255, b / 255];
  const max = Math.max(rr, gg, bb);
  const d = max - Math.min(rr, gg, bb);
  return { h: hueOf(rr, gg, bb, max, d), s: max === 0 ? 0 : (d / max) * 100, v: max * 100 };
}

export function hsvToRgb({ h, s, v }: Hsv): Rgb {
  const [ss, vv] = [clamp(s, 0, 100) / 100, clamp(v, 0, 100) / 100];
  const f = (n: number) => {
    const k = (n + (((h % 360) + 360) % 360) / 60) % 6;
    return (vv - vv * ss * Math.max(0, Math.min(k, 4 - k, 1))) * 255;
  };
  return { r: Math.round(f(5)), g: Math.round(f(3)), b: Math.round(f(1)) };
}

export function rgbToHsl({ r, g, b }: Rgb): Hsl {
  const [rr, gg, bb] = [r / 255, g / 255, b / 255];
  const max = Math.max(rr, gg, bb);
  const min = Math.min(rr, gg, bb);
  const d = max - min;
  const l = (max + min) / 2;
  const s = d === 0 ? 0 : d / (1 - Math.abs(2 * l - 1));
  return { h: hueOf(rr, gg, bb, max, d), s: s * 100, l: l * 100 };
}

export function hslToRgb({ h, s, l }: Hsl): Rgb {
  const [ss, ll] = [clamp(s, 0, 100) / 100, clamp(l, 0, 100) / 100];
  const a = ss * Math.min(ll, 1 - ll);
  const f = (n: number) => {
    const k = (n + (((h % 360) + 360) % 360) / 30) % 12;
    return (ll - a * Math.max(-1, Math.min(k - 3, 9 - k, 1))) * 255;
  };
  return { r: Math.round(f(0)), g: Math.round(f(8)), b: Math.round(f(4)) };
}

/** The hue of a color whose channels are 0–1; 0 for grays. */
function hueOf(r: number, g: number, b: number, max: number, d: number): number {
  if (d === 0) return 0;
  const h = max === r ? ((g - b) / d) % 6 : max === g ? (b - r) / d + 2 : (r - g) / d + 4;
  return (h * 60 + 360) % 360;
}

/** HSL of an HSV color, keeping its hue and saturation where RGB has none (grays, black). */
export function hsvToHsl({ h, s, v }: Hsv): Hsl {
  const [ss, vv] = [s / 100, v / 100];
  const l = vv * (1 - ss / 2);
  const sl = l === 0 || l === 1 ? 0 : (vv - l) / Math.min(l, 1 - l);
  return { h, s: sl * 100, l: l * 100 };
}

/** HSV of an HSL color, keeping its hue. */
export function hslToHsv({ h, s, l }: Hsl): Hsv {
  const [ss, ll] = [clamp(s, 0, 100) / 100, clamp(l, 0, 100) / 100];
  const v = ll + ss * Math.min(ll, 1 - ll);
  return { h, s: v === 0 ? 0 : 2 * (1 - ll / v) * 100, v: v * 100 };
}
