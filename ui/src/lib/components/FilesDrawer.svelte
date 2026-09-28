<script lang="ts">
  // Files of the active session over SFTP, next to the terminal (the terminal narrows,
  // nothing is covered). Double-click opens a folder or downloads a file; files dropped
  // here are uploaded into the folder shown.
  import { keys } from '../actions';
  import { t } from '../i18n.svelte';
  import { app, type MenuEntry } from '../state/app.svelte';
  import { files, formatDate, formatSize, isDirLike, joinPath, type Transfer } from '../state/files.svelte';
  import type { Tab } from '../state/sessions.svelte';
  import { sessions } from '../state/sessions.svelte';
  import { toasts } from '../state/toasts.svelte';
  import type { SftpEntry } from '../types';
  import Icon from './Icon.svelte';

  interface Props {
    tab: Tab;
  }
  let { tab }: Props = $props();

  let list = $state<HTMLDivElement>();
  let picker = $state<HTMLInputElement>();
  let pathInput = $state('');
  let dragging = $state(false);

  const connected = $derived(tab.status === 'connected' && tab.sessionId != null);
  const view = $derived(files.views[tab.key]);
  const entries = $derived(
    (view?.entries ?? []).filter((e) => app.settings.filesShowHidden || !e.name.startsWith('.')),
  );
  const dirName = $derived(view?.path ? view.path.replace(/\/+$/, '').split('/').pop() || '/' : '');

  // Load the tab's folder when shown; after a reconnect, retry what failed.
  $effect(() => {
    if (!connected) return;
    if (!files.views[tab.key]) files.ensure(tab);
    else if (files.views[tab.key].error) void files.goHome(tab);
  });

  $effect(() => {
    pathInput = view?.path ?? '';
  });

  // Forget folders of closed tabs.
  $effect(() => {
    const open = new Set(sessions.tabs.map((x) => x.key));
    for (const key of Object.keys(files.views)) if (!open.has(key)) files.forget(key);
  });

  function select(name: string | null) {
    if (view) view.selected = name;
  }

  function activate(e: SftpEntry) {
    if (!view?.path) return;
    if (isDirLike(e)) void files.openDir(tab, joinPath(view.path, e.name));
    else void files.download(tab, e);
  }

  function go(ev: SubmitEvent) {
    ev.preventDefault();
    const path = pathInput.trim();
    if (path) void files.goTo(tab, path);
  }

  function entryMenu(ev: MouseEvent, e: SftpEntry) {
    ev.preventDefault();
    ev.stopPropagation();
    select(e.name);
    const items: MenuEntry[] = [
      isDirLike(e)
        ? { label: t('files.openItem'), icon: 'folder', action: () => activate(e) }
        : { label: t('files.download'), icon: 'download', action: () => void files.download(tab, e) },
    ];
    if (isDirLike(e)) items.push({ label: t('files.download'), icon: 'download', action: () => void files.download(tab, e) });
    items.push(
      { label: t('files.rename'), icon: 'edit', hint: 'F2', action: () => void files.rename(tab, e) },
      { label: t('files.copyPath'), icon: 'copy', action: () => copyPath(e) },
      'separator',
      { label: t('common.delete'), icon: 'trash', danger: true, hint: 'Del', action: () => void files.remove(tab, e) },
    );
    app.showMenu(ev.clientX, ev.clientY, items);
  }

  function folderMenu(ev: MouseEvent) {
    ev.preventDefault();
    select(null);
    app.showMenu(ev.clientX, ev.clientY, [
      { label: t('files.upload'), icon: 'upload', action: () => picker?.click() },
      { label: t('files.newFolder'), icon: 'folder', action: () => void files.mkdir(tab) },
      { label: t('files.refresh'), icon: 'refresh', action: () => files.refresh(tab) },
      'separator',
      {
        label: t('files.showHidden'),
        icon: app.settings.filesShowHidden ? 'check' : undefined,
        action: () => app.update({ filesShowHidden: !app.settings.filesShowHidden }),
      },
    ]);
  }

  async function copyPath(e: SftpEntry) {
    if (!view?.path) return;
    const path = joinPath(view.path, e.name);
    await navigator.clipboard.writeText(path).catch(() => {});
    toasts.show(t('common.copied', { text: path }));
  }

  function onkeydown(ev: KeyboardEvent) {
    if (!view) return;
    const index = entries.findIndex((e) => e.name === view.selected);
    const current = index >= 0 ? entries[index] : undefined;
    const move = (to: number) => {
      const next = entries[Math.max(0, Math.min(entries.length - 1, to))];
      if (next) {
        select(next.name);
        list?.querySelector<HTMLElement>(`[data-name="${CSS.escape(next.name)}"]`)?.scrollIntoView({ block: 'nearest' });
      }
    };
    switch (ev.key) {
      case 'ArrowDown':
        move(index + 1);
        break;
      case 'ArrowUp':
        move(index - 1);
        break;
      case 'Enter':
        if (current) activate(current);
        break;
      case 'Backspace':
        files.up(tab);
        break;
      case 'Delete':
        if (current) void files.remove(tab, current);
        break;
      case 'F2':
        if (current) void files.rename(tab, current);
        break;
      case 'Escape':
        sessions.focusActive();
        break;
      default:
        return;
    }
    ev.preventDefault();
  }

  function pick() {
    const chosen = Array.from(picker?.files ?? []);
    if (picker) picker.value = '';
    void files.upload(tab, chosen);
  }

  function hasFiles(ev: DragEvent) {
    return ev.dataTransfer?.types.includes('Files') ?? false;
  }

  function ondragover(ev: DragEvent) {
    if (!hasFiles(ev) || !view?.path) return;
    ev.preventDefault();
    if (ev.dataTransfer) ev.dataTransfer.dropEffect = 'copy';
    dragging = true;
  }

  function ondrop(ev: DragEvent) {
    dragging = false;
    if (!hasFiles(ev) || !ev.dataTransfer) return;
    ev.preventDefault();
    const items = Array.from(ev.dataTransfer.items);
    const folders = items.some((item) => item.webkitGetAsEntry?.()?.isDirectory);
    const dropped = items
      .filter((item) => item.kind === 'file' && !item.webkitGetAsEntry?.()?.isDirectory)
      .map((item) => item.getAsFile())
      .filter((f): f is File => !!f);
    if (folders) toasts.show(t('files.noFolders'));
    void files.upload(tab, dropped);
  }

  function percent(tr: Transfer) {
    return tr.total ? Math.min(100, Math.round((tr.done / tr.total) * 100)) : 0;
  }

  function status(tr: Transfer) {
    switch (tr.status) {
      case 'done':
        return t('files.done');
      case 'failed':
        return t('files.failed');
      case 'cancelled':
        return t('files.cancelled');
      default:
        return tr.total ? `${percent(tr)}%` : formatSize(tr.done);
    }
  }

  // ---- width -------------------------------------------------------------------
  let width = $derived(app.settings.filesWidth);
  let resizing = $state(false);

  function startResize(e: PointerEvent) {
    e.preventDefault();
    resizing = true;
    const startX = e.clientX;
    const startWidth = app.settings.filesWidth;
    const target = e.currentTarget as HTMLElement;
    target.setPointerCapture(e.pointerId);
    const move = (ev: PointerEvent) => {
      width = Math.round(Math.min(640, Math.max(300, startWidth - (ev.clientX - startX))));
    };
    const up = () => {
      resizing = false;
      target.removeEventListener('pointermove', move);
      target.removeEventListener('pointerup', up);
      app.update({ filesWidth: width });
    };
    target.addEventListener('pointermove', move);
    target.addEventListener('pointerup', up);
  }
</script>

<aside class="files" style:width="{width}px" aria-label={t('files.title')}>
  <!-- svelte-ignore a11y_no_static_element_interactions -->
  <div class="resizer" class:resizing onpointerdown={startResize} ondblclick={() => app.update({ filesWidth: 380 })}></div>

  <header>
    <div class="top">
      <span class="title">{t('files.title')}</span>
      <span class="host" title={tab.subtitle}>{tab.title}</span>
      <span class="spacer"></span>
      <button class="icon-btn small" title={t('files.upload')} aria-label={t('files.upload')} disabled={!view?.path} onclick={() => picker?.click()}>
        <Icon name="upload" size={15} />
      </button>
      <button class="icon-btn small" title={t('files.newFolder')} aria-label={t('files.newFolder')} disabled={!view?.path} onclick={() => files.mkdir(tab)}>
        <Icon name="folderPlus" size={15} />
      </button>
      <button class="icon-btn small" title={t('files.refresh')} aria-label={t('files.refresh')} disabled={!connected} onclick={() => files.refresh(tab)}>
        <Icon name="refresh" size={14} />
      </button>
      <button class="icon-btn small" title="{t('files.close')} ({keys.files()})" aria-label={t('files.close')} onclick={() => (files.open = false)}>
        <Icon name="x" size={14} />
      </button>
    </div>
    <form class="path" onsubmit={go}>
      <button type="button" class="icon-btn small" title={t('files.up')} aria-label={t('files.up')} disabled={!view?.path || view.path === '/'} onclick={() => files.up(tab)}>
        <Icon name="arrowUp" size={14} />
      </button>
      <!-- A path that failed to open stays for correction until the field loses focus. -->
      <input
        class="input mono"
        bind:value={pathInput}
        spellcheck="false"
        autocomplete="off"
        aria-label={t('files.path')}
        disabled={!connected}
        onblur={() => (pathInput = view?.path ?? '')}
        onkeydown={(e) => e.key === 'Escape' && list?.focus()}
      />
    </form>
  </header>

  <!-- svelte-ignore a11y_no_noninteractive_tabindex -->
  <div
    class="list"
    class:dragging
    bind:this={list}
    tabindex="0"
    role="listbox"
    aria-label={t('files.title')}
    {onkeydown}
    oncontextmenu={(e) => connected && view?.path && folderMenu(e)}
    {ondragover}
    ondragleave={(e) => {
      if (e.currentTarget === e.target) dragging = false;
    }}
    {ondrop}
  >
    {#if !connected}
      <p class="note">{t('files.notConnected')}</p>
    {:else if !view || (view.loading && !view.path)}
      <p class="note">{t('files.loading')}</p>
    {:else if view.error}
      <div class="note error">
        <p>{view.error}</p>
        <button class="btn" onclick={() => files.goHome(tab)}>{t('files.retry')}</button>
      </div>
    {:else}
      {#each entries as e (e.name)}
        <div
          class="row"
          class:selected={view.selected === e.name}
          data-name={e.name}
          role="option"
          tabindex="-1"
          aria-selected={view.selected === e.name}
          title={e.permissions ? `${e.name}\n${e.permissions}` : e.name}
          onclick={() => select(e.name)}
          ondblclick={() => activate(e)}
          onkeydown={() => {}}
          oncontextmenu={(ev) => entryMenu(ev, e)}
        >
          <span class="icon" class:dir={isDirLike(e)}>
            <Icon name={isDirLike(e) ? 'folder' : e.kind === 'link' ? 'link' : 'file'} size={15} />
          </span>
          <span class="name">{e.name}</span>
          <span class="size">{isDirLike(e) ? '' : formatSize(e.size)}</span>
          <span class="date">{formatDate(e.modified)}</span>
        </div>
      {:else}
        <p class="note">{t('files.empty')}</p>
      {/each}
    {/if}
    {#if dragging}
      <div class="drop">{t('files.drop', { dir: dirName })}</div>
    {/if}
  </div>

  {#if files.transfers.length}
    <footer>
      <div class="thead">
        <span>{t('files.transfers')}</span>
        {#if files.transfers.length > files.running}
          <button class="link" onclick={() => files.clearFinished()}>{t('files.clearDone')}</button>
        {/if}
      </div>
      <div class="tlist">
        {#each files.transfers as tr (tr.id)}
          <div class="transfer" class:failed={tr.status === 'failed'} title={tr.error ?? tr.localPath ?? tr.name}>
            <span class="ticon"><Icon name={tr.direction === 'down' ? 'download' : 'upload'} size={13} /></span>
            <span class="tname">{tr.name}</span>
            <span class="tstate">{status(tr)}</span>
            {#if tr.status === 'running'}
              <button class="icon-btn small" title={t('common.cancel')} aria-label={t('common.cancel')} onclick={() => files.cancel(tr)}>
                <Icon name="x" size={12} />
              </button>
            {:else if tr.status === 'done' && tr.localPath}
              <button class="icon-btn small" title={t('files.reveal')} aria-label={t('files.reveal')} onclick={() => files.reveal(tr)}>
                <Icon name="folder" size={13} />
              </button>
            {/if}
            {#if tr.status === 'running'}
              <span class="tbar"><span style:width="{tr.total ? percent(tr) : 30}%" class:indeterminate={!tr.total}></span></span>
            {/if}
          </div>
        {/each}
      </div>
    </footer>
  {/if}

  <input bind:this={picker} type="file" multiple hidden onchange={pick} />
</aside>

<style>
  .files {
    position: relative;
    flex: none;
    display: flex;
    flex-direction: column;
    min-width: 0;
    border-left: 1px solid var(--border-soft);
    background: var(--bg);
  }
  .resizer {
    position: absolute;
    top: 0;
    left: -3px;
    width: 6px;
    height: 100%;
    cursor: col-resize;
    z-index: 3;
  }
  .resizer:hover,
  .resizer.resizing {
    background: color-mix(in srgb, var(--text-2) 18%, transparent);
  }
  header {
    display: flex;
    flex-direction: column;
    gap: 6px;
    padding: 8px 8px 8px 12px;
    border-bottom: 1px solid var(--border-soft);
  }
  .top {
    display: flex;
    align-items: center;
    gap: 2px;
    min-width: 0;
  }
  .title {
    font-weight: 600;
    margin-right: 6px;
  }
  .host {
    min-width: 0;
    overflow: hidden;
    white-space: nowrap;
    text-overflow: ellipsis;
    color: var(--text-2);
    font-size: 12px;
  }
  .spacer {
    flex: 1;
  }
  .path {
    display: flex;
    align-items: center;
    gap: 4px;
  }
  .path .input {
    flex: 1;
    min-width: 0;
    height: 28px;
    font-size: 12px;
  }
  .list {
    position: relative;
    flex: 1;
    min-height: 0;
    overflow: auto;
    padding: 4px;
    outline: none;
  }
  .list.dragging {
    background: color-mix(in srgb, var(--green) 6%, var(--bg));
  }
  .row {
    display: grid;
    grid-template-columns: 18px minmax(0, 1fr) auto auto;
    align-items: center;
    gap: 8px;
    height: 28px;
    padding: 0 8px;
    border-radius: var(--radius-sm);
    cursor: default;
    user-select: none;
  }
  .row:hover {
    background: var(--hover);
  }
  .row.selected {
    background: var(--active);
  }
  .icon {
    display: grid;
    color: var(--text-3);
  }
  .icon.dir {
    color: var(--text-2);
  }
  .name {
    overflow: hidden;
    white-space: nowrap;
    text-overflow: ellipsis;
  }
  .size,
  .date {
    color: var(--text-2);
    font-size: 11.5px;
    white-space: nowrap;
    font-variant-numeric: tabular-nums;
  }
  .date {
    min-width: 84px;
    text-align: right;
  }
  .note {
    margin: 0;
    padding: 24px 12px;
    text-align: center;
    color: var(--text-2);
  }
  .note.error {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 10px;
    color: var(--red);
  }
  .note.error p {
    margin: 0;
  }
  .drop {
    position: absolute;
    inset: 6px;
    display: grid;
    place-items: center;
    padding: 12px;
    border: 1.5px dashed color-mix(in srgb, var(--green) 60%, transparent);
    border-radius: var(--radius);
    background: color-mix(in srgb, var(--bg) 80%, transparent);
    color: var(--text);
    text-align: center;
    pointer-events: none;
  }
  footer {
    border-top: 1px solid var(--border-soft);
    padding: 6px 8px 8px;
    max-height: 40%;
    display: flex;
    flex-direction: column;
    min-height: 0;
  }
  .thead {
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: 0 4px 4px;
    color: var(--text-2);
    font-size: 11.5px;
    font-weight: 600;
  }
  .link {
    border: 0;
    background: none;
    padding: 0;
    color: var(--text-2);
    font-size: 11.5px;
  }
  .link:hover {
    color: var(--text);
  }
  .tlist {
    overflow: auto;
    min-height: 0;
  }
  .transfer {
    display: grid;
    grid-template-columns: 16px minmax(0, 1fr) auto 22px;
    align-items: center;
    column-gap: 6px;
    row-gap: 3px;
    padding: 4px;
    font-size: 12px;
  }
  .ticon {
    display: grid;
    color: var(--text-2);
  }
  .tname {
    overflow: hidden;
    white-space: nowrap;
    text-overflow: ellipsis;
  }
  .tstate {
    color: var(--text-2);
    font-size: 11.5px;
    font-variant-numeric: tabular-nums;
  }
  .transfer.failed .tstate {
    color: var(--red);
  }
  .tbar {
    grid-column: 2 / 4;
    position: relative;
    height: 3px;
    border-radius: 2px;
    background: var(--border-soft);
    overflow: hidden;
  }
  .tbar span {
    position: absolute;
    inset: 0 auto 0 0;
    border-radius: 2px;
    background: var(--green);
    transition: width 0.15s linear;
  }
  .tbar span.indeterminate {
    animation: slide 1.2s var(--ease) infinite;
  }
  @keyframes slide {
    from {
      transform: translateX(-100%);
    }
    to {
      transform: translateX(340%);
    }
  }
</style>
