// Color math of the color picker (src/lib/color.ts).
// Run with `npm test` (Node's test runner; Node 22.18+ reads TypeScript itself).

import assert from 'node:assert/strict';
import { test } from 'node:test';
import { hexToRgb, hslToHsv, hslToRgb, hsvToHsl, hsvToRgb, parseHex, rgbToHex, rgbToHsl, rgbToHsv, type Rgb } from '../src/lib/color.ts';

const round = <T extends object>(o: T): T =>
  Object.fromEntries(Object.entries(o).map(([k, v]) => [k, Math.round(v as number)])) as T;

test('hex as typed', () => {
  assert.equal(parseHex('295cc0'), '#295cc0');
  assert.equal(parseHex('#295CC0'), '#295cc0');
  assert.equal(parseHex(' fff '), '#ffffff');
  assert.equal(parseHex('#0a3'), '#00aa33');
  assert.equal(parseHex('29cc0'), null);
  assert.equal(parseHex('#ggg'), null);
  assert.equal(parseHex(''), null);
});

test('hex and RGB', () => {
  assert.deepEqual(hexToRgb('#295cc0'), { r: 41, g: 92, b: 192 });
  assert.equal(rgbToHex({ r: 41, g: 92, b: 192 }), '#295cc0');
  assert.equal(rgbToHex({ r: 300, g: -4, b: 7.6 }), '#ff0008');
});

test('HSL of a color', () => {
  assert.deepEqual(round(rgbToHsl({ r: 41, g: 92, b: 192 })), { h: 220, s: 65, l: 46 });
  assert.deepEqual(round(rgbToHsl({ r: 128, g: 128, b: 128 })), { h: 0, s: 0, l: 50 });
});

test('HSV of a color', () => {
  assert.deepEqual(round(rgbToHsv({ r: 255, g: 0, b: 0 })), { h: 0, s: 100, v: 100 });
  assert.deepEqual(round(rgbToHsv({ r: 0, g: 0, b: 0 })), { h: 0, s: 0, v: 0 });
  assert.deepEqual(round(rgbToHsv({ r: 41, g: 92, b: 192 })), { h: 220, s: 79, v: 75 });
});

test('there and back again', () => {
  const colors: Rgb[] = [
    { r: 41, g: 92, b: 192 },
    { r: 243, g: 94, b: 98 },
    { r: 255, g: 202, b: 0 },
    { r: 0, g: 0, b: 0 },
    { r: 255, g: 255, b: 255 },
    { r: 134, g: 134, b: 254 },
  ];
  for (const rgb of colors) {
    assert.deepEqual(hsvToRgb(rgbToHsv(rgb)), rgb);
    assert.deepEqual(hslToRgb(rgbToHsl(rgb)), rgb);
  }
  assert.deepEqual(hsvToRgb({ h: 360, s: 100, v: 100 }), { r: 255, g: 0, b: 0 });
  assert.deepEqual(hslToRgb({ h: 120, s: 100, l: 50 }), { r: 0, g: 255, b: 0 });
});

test('HSV and HSL keep the hue of grays', () => {
  assert.deepEqual(round(hsvToHsl({ h: 200, s: 0, v: 50 })), { h: 200, s: 0, l: 50 });
  const blue = { r: 41, g: 92, b: 192 };
  assert.deepEqual(round(hsvToHsl(rgbToHsv(blue))), round(rgbToHsl(blue)));
  assert.deepEqual(round(hslToHsv({ h: 220, s: 65, l: 46 })), { h: 220, s: 79, v: 76 });
  assert.deepEqual(round(hslToHsv({ h: 90, s: 50, l: 0 })), { h: 90, s: 0, v: 0 });
  assert.deepEqual(round(hsvToHsl(hslToHsv({ h: 300, s: 40, l: 70 }))), { h: 300, s: 40, l: 70 });
});
