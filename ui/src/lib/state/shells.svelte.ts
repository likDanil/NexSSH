// Shells for local terminals: found by the backend, chosen in the settings.

import { api } from '../api';
import { t } from '../i18n.svelte';
import type { ShellProfile } from '../types';

/** `localShell` of the settings for a command line of the user's own. */
export const CUSTOM_SHELL = 'custom';

class ShellsState {
  list = $state<ShellProfile[]>([]);
  windowsBuild = $state<number | null>(null);
  loaded = $state(false);
  #loading: Promise<void> | null = null;

  /** Looks for shells once, or again with `refresh` (a WSL distribution may have been added). */
  load(refresh = false): Promise<void> {
    if (!this.#loading || refresh) {
      this.#loading = api
        .localShells()
        .then((found) => {
          this.list = found.shells;
          this.windowsBuild = found.windowsBuild ?? null;
        })
        .catch((e) => console.error('local shells', e))
        .finally(() => (this.loaded = true));
    }
    return this.#loading;
  }

  get default(): ShellProfile | undefined {
    return this.list.find((s) => s.default) ?? this.list[0];
  }

  byId(id: string): ShellProfile | undefined {
    return this.list.find((s) => s.id === id);
  }
}

export const shells = new ShellsState();

/** A shell's name in the interface's language. */
export function shellName(shell: ShellProfile): string {
  switch (shell.kind) {
    case 'cmd':
      return t('local.cmd');
    case 'wsl':
      return `WSL — ${shell.name}`;
    default:
      return shell.name;
  }
}

/** A name for a settings value that names no installed shell: `wsl:Ubuntu` → WSL — Ubuntu. */
export function missingShellName(id: string): string {
  if (id.startsWith('wsl:')) return `WSL — ${id.slice(4)}`;
  return id.split(/[\\/]/).pop() || id;
}

/** A name for a command line: its program's file name without `.exe` (`zsh -l` → zsh). */
export function commandName(line: string): string {
  const trimmed = line.trim();
  const program = trimmed.startsWith('"') || trimmed.startsWith("'")
    ? trimmed.slice(1).split(trimmed[0])[0]
    : trimmed.split(/\s+/)[0];
  const file = program.split(/[\\/]/).pop() || program;
  return file.replace(/\.(exe|cmd|bat|com)$/i, '') || trimmed;
}
