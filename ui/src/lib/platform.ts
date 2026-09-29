// Platform helpers. The OS comes from the backend (`app_info`), with a UA fallback.

export type Os = 'windows' | 'macos' | 'linux' | 'other';

let os: Os = /Mac/i.test(navigator.platform) ? 'macos' : /Win/i.test(navigator.platform) ? 'windows' : 'linux';

export function setOs(value: string) {
  os = value === 'windows' || value === 'macos' || value === 'linux' ? value : 'other';
  document.documentElement.dataset.os = os;
}

export function currentOs(): Os {
  return os;
}

export const isMac = () => os === 'macos';

/** Cmd on macOS, Ctrl elsewhere. */
export function mod(e: KeyboardEvent | MouseEvent): boolean {
  return isMac() ? e.metaKey : e.ctrlKey;
}

/** Keys that type punctuation, by their place on a US keyboard. */
const CODE_KEYS: Record<string, string> = {
  Backquote: '`',
  Minus: '-',
  Equal: '=',
  BracketLeft: '[',
  BracketRight: ']',
  Backslash: '\\',
  Semicolon: ';',
  Quote: "'",
  Comma: ',',
  Period: '.',
  Slash: '/',
};

/**
 * The key of a shortcut: the character typed (lower case), or the key's name (`Tab`, `F11`).
 * A key typing a non-Latin character (Cyrillic, Greek…) counts as the US key in its place, so
 * Ctrl+Shift+K works with any keyboard layout.
 */
export function shortcutKey(e: KeyboardEvent): string {
  if (e.key.length === 1 && e.key.charCodeAt(0) < 128) return e.key.toLowerCase();
  if (/^Key[A-Z]$/.test(e.code)) return e.code.slice(3).toLowerCase();
  if (/^Digit\d$/.test(e.code)) return e.code.slice(5);
  return CODE_KEYS[e.code] ?? (e.key.length === 1 ? e.key.toLowerCase() : e.key);
}

/** Human readable shortcut, e.g. `shortcut('Shift', 'K')` → "Ctrl+Shift+K" / "⌘⇧K". */
export function shortcut(...keys: string[]): string {
  const mac = isMac();
  const names = keys.map((k) => {
    if (k === 'Mod') return mac ? '⌘' : 'Ctrl';
    if (k === 'Shift') return mac ? '⇧' : 'Shift';
    if (k === 'Alt') return mac ? '⌥' : 'Alt';
    if (k === 'Ctrl') return mac ? '⌃' : 'Ctrl';
    return k;
  });
  return mac ? names.join('') : names.join('+');
}
