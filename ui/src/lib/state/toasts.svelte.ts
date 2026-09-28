export type ToastKind = 'info' | 'success' | 'error';

export interface Toast {
  id: number;
  kind: ToastKind;
  text: string;
}

class Toasts {
  items = $state<Toast[]>([]);
  #next = 1;

  show(text: string, kind: ToastKind = 'info', ms = 3600) {
    const id = this.#next++;
    this.items.push({ id, kind, text });
    if (this.items.length > 4) this.items.shift();
    setTimeout(() => this.dismiss(id), kind === 'error' ? ms * 1.6 : ms);
  }

  error(text: string) {
    this.show(text, 'error');
  }

  dismiss(id: number) {
    this.items = this.items.filter((t) => t.id !== id);
  }
}

export const toasts = new Toasts();
