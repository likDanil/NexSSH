<script lang="ts" module>
  // Hand-drawn 24×24 stroke icons: no icon font or icon library in the bundle.
  const ICONS: Record<string, string> = {
    search: '<circle cx="11" cy="11" r="6.5"/><path d="m20 20-4.2-4.2"/>',
    plus: '<path d="M12 5v14M5 12h14"/>',
    minus: '<path d="M5 12h14"/>',
    text: '<path d="M5.5 7.5v-2h13v2M12 5.5v13M9 18.5h6"/>',
    x: '<path d="M17 7 7 17M7 7l10 10"/>',
    chevronRight: '<path d="m10 7 5 5-5 5"/>',
    chevronDown: '<path d="m7 10 5 5 5-5"/>',
    more: '<circle cx="6" cy="12" r="1.3" fill="currentColor" stroke="none"/><circle cx="12" cy="12" r="1.3" fill="currentColor" stroke="none"/><circle cx="18" cy="12" r="1.3" fill="currentColor" stroke="none"/>',
    settings: '<path d="M4 7h9M17 7h3M4 17h3M11 17h9"/><circle cx="15" cy="7" r="2"/><circle cx="9" cy="17" r="2"/>',
    zap: '<path d="M13 3 5 13.5h6L10 21l8-10.5h-6L13 3Z"/>',
    server: '<rect x="4" y="4" width="16" height="7" rx="2"/><rect x="4" y="13" width="16" height="7" rx="2"/><path d="M8 7.5h.01M8 16.5h.01"/>',
    terminal: '<rect x="3" y="4.5" width="18" height="15" rx="3"/><path d="m7.5 9.5 3 2.5-3 2.5M12.5 15h4"/>',
    refresh: '<path d="M19.5 11A7.5 7.5 0 0 0 6 6.5L4.5 8"/><path d="M4.5 4v4h4"/><path d="M4.5 13A7.5 7.5 0 0 0 18 17.5l1.5-1.5"/><path d="M19.5 20v-4h-4"/>',
    power: '<path d="M12 3.5v8"/><path d="M6.6 6.8a7.5 7.5 0 1 0 10.8 0"/>',
    fullscreen: '<path d="M4 9V5.5A1.5 1.5 0 0 1 5.5 4H9M15 4h3.5A1.5 1.5 0 0 1 20 5.5V9M20 15v3.5a1.5 1.5 0 0 1-1.5 1.5H15M9 20H5.5A1.5 1.5 0 0 1 4 18.5V15"/>',
    sidebar: '<rect x="3" y="4.5" width="18" height="15" rx="3"/><path d="M9.5 4.5v15"/>',
    import: '<path d="M12 4v10.5M8 10.5l4 4 4-4"/><path d="M4.5 15v2.5a2 2 0 0 0 2 2h11a2 2 0 0 0 2-2V15"/>',
    edit: '<path d="M4.5 19.5h3.8L18.6 9.2a2.2 2.2 0 0 0-3.8-3.8L4.5 15.7v3.8Z"/><path d="m13.6 6.6 3.8 3.8"/>',
    trash: '<path d="M4.5 7h15M10 11v5.5M14 11v5.5M6.5 7l.9 11.1a2 2 0 0 0 2 1.9h5.2a2 2 0 0 0 2-1.9L17.5 7M9.5 7V5.5a1 1 0 0 1 1-1h3a1 1 0 0 1 1 1V7"/>',
    copy: '<rect x="8.5" y="8.5" width="11" height="11" rx="2"/><path d="M15.5 8.5V6.5a2 2 0 0 0-2-2h-7a2 2 0 0 0-2 2v7a2 2 0 0 0 2 2h2"/>',
    paste: '<rect x="5" y="4.5" width="14" height="16" rx="2"/><path d="M9 4.5V4a1 1 0 0 1 1-1h4a1 1 0 0 1 1 1v.5M9 10h6M9 14h4"/>',
    key: '<circle cx="8" cy="15" r="3.8"/><path d="m10.8 12.2 8.2-8.2M15.5 7.5l2.5 2.5M13.5 9.5l2 2"/>',
    lock: '<rect x="5" y="10.5" width="14" height="10" rx="2.5"/><path d="M8.5 10.5V8a3.5 3.5 0 0 1 7 0v2.5"/>',
    user: '<circle cx="12" cy="8.5" r="3.5"/><path d="M5 19.5c.9-3.4 3.7-5.5 7-5.5s6.1 2.1 7 5.5"/>',
    shield: '<path d="M12 3.5 5.5 6v5.2c0 4.1 2.7 7.6 6.5 9.3 3.8-1.7 6.5-5.2 6.5-9.3V6L12 3.5Z"/>',
    alert: '<path d="M12 4 3.5 19h17L12 4Z"/><path d="M12 10v4M12 16.8h.01"/>',
    forward: '<path d="M4 8h13M13.5 4.5 17 8l-3.5 3.5"/><path d="M20 16H7M10.5 12.5 7 16l3.5 3.5"/>',
    check: '<path d="m5 12.5 4.5 4.5L19 7.5"/>',
    folder: '<path d="M4 7.5A2 2 0 0 1 6 5.5h3.6l2 2H18a2 2 0 0 1 2 2v7a2 2 0 0 1-2 2H6a2 2 0 0 1-2-2v-9Z"/>',
    theme: '<circle cx="12" cy="12" r="7.5"/><path d="M12 4.5a7.5 7.5 0 0 1 0 15Z" fill="currentColor" stroke="none"/>',
    upload: '<path d="M12 15.5V4.5M8 8.5l4-4 4 4"/><path d="M4.5 15v2.5a2 2 0 0 0 2 2h11a2 2 0 0 0 2-2V15"/>',
    download: '<path d="M12 4.5v11M8 11.5l4 4 4-4"/><path d="M4.5 15v2.5a2 2 0 0 0 2 2h11a2 2 0 0 0 2-2V15"/>',
    folderPlus: '<path d="M4 7.5A2 2 0 0 1 6 5.5h3.6l2 2H18a2 2 0 0 1 2 2v7a2 2 0 0 1-2 2H6a2 2 0 0 1-2-2v-9Z"/><path d="M12 10.5v5M9.5 13h5"/>',
    arrowUp: '<path d="M12 19V5.5M6.5 11 12 5.5 17.5 11"/>',
    file: '<path d="M7 3.5h6.5L18 8v11a1.5 1.5 0 0 1-1.5 1.5h-9A1.5 1.5 0 0 1 6 19V5a1.5 1.5 0 0 1 1-1.5Z"/><path d="M13.5 3.5V8H18"/>',
    link: '<path d="M10 14a4 4 0 0 0 5.7 0l3-3a4 4 0 0 0-5.7-5.7l-1 1"/><path d="M14 10a4 4 0 0 0-5.7 0l-3 3a4 4 0 0 0 5.7 5.7l1-1"/>',
    update: '<circle cx="12" cy="12" r="8"/><path d="M12 15.5V8.8M8.9 11.6 12 8.5l3.1 3.1"/>',
    folder2: '<path d="M4 7.5A2 2 0 0 1 6 5.5h3.6l2 2H18a2 2 0 0 1 2 2v7a2 2 0 0 1-2 2H6a2 2 0 0 1-2-2v-9Z"/><path d="M4 10.5h16"/>',
    globe: '<circle cx="12" cy="12" r="8"/><path d="M4 12h16"/><path d="M12 4c2.2 2.3 3.3 5 3.3 8s-1.1 5.7-3.3 8c-2.2-2.3-3.3-5-3.3-8s1.1-5.7 3.3-8Z"/>',
    eraser: '<path d="m7 20 11.2-11.2a2 2 0 0 0 0-2.8l-2.2-2.2a2 2 0 0 0-2.8 0L3.8 13.2a2 2 0 0 0 0 2.8L7.8 20H20"/><path d="m9 9 6 6"/>',
    duplicate: '<rect x="8.5" y="8.5" width="11" height="11" rx="2"/><path d="M14 11.5v5M11.5 14h5"/><path d="M15.5 8.5V6.5a2 2 0 0 0-2-2h-7a2 2 0 0 0-2 2v7a2 2 0 0 0 2 2h2"/>',
    enter: '<path d="M19 5v6a3 3 0 0 1-3 3H6"/><path d="m9.5 10.5-3.5 3.5 3.5 3.5"/>',
    home: '<path d="M4.5 10.5 12 4.5l7.5 6V19a1.5 1.5 0 0 1-1.5 1.5h-3.5V15h-5v5.5H6A1.5 1.5 0 0 1 4.5 19v-8.5Z"/>',
    winMin: '<path d="M6 12h12"/>',
    winMax: '<rect x="6" y="6" width="12" height="12" rx="1.2"/>',
    winRestore: '<rect x="6" y="8.5" width="9.5" height="9.5" rx="1.2"/><path d="M8.5 8.5V7.2A1.2 1.2 0 0 1 9.7 6h7.1A1.2 1.2 0 0 1 18 7.2v7.1a1.2 1.2 0 0 1-1.2 1.2h-1.3"/>',
    winClose: '<path d="M6.5 6.5l11 11M17.5 6.5l-11 11"/>',
  };
</script>

<script lang="ts">
  interface Props {
    name: string;
    size?: number;
    stroke?: number;
  }
  let { name, size = 16, stroke = 1.7 }: Props = $props();
</script>

<svg
  width={size}
  height={size}
  viewBox="0 0 24 24"
  fill="none"
  stroke="currentColor"
  stroke-width={stroke}
  stroke-linecap="round"
  stroke-linejoin="round"
  aria-hidden="true"
>
  {@html ICONS[name] ?? ''}
</svg>

<style>
  svg {
    flex: none;
    display: block;
  }
</style>
