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
