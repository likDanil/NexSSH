<script lang="ts">
  // Files of the active session over SFTP, next to the terminal (the terminal narrows,
  // nothing is covered). Double-click opens a folder, and downloads a file or opens it in the
  // editor (as the settings say; F4 edits); Ctrl/Shift+click
  // and Ctrl+A select several entries for one command. Files and folders dropped here are
  // uploaded into the folder shown, or into the folder under the pointer.
  import { listen } from '@tauri-apps/api/event';
  import { keys } from '../actions';
  import { errorMessage } from '../api';
  import { i18n, t, tn, type MessageKey } from '../i18n.svelte';
  import { currentOs, mod, shortcut } from '../platform';
  import { app, type MenuEntry } from '../state/app.svelte';
  import { edits } from '../state/edits.svelte';
  import {
    files,
    formatDate,
    formatDuration,
    formatSize,
    isDirLike,
    joinPath,
    pathsOfDrop,
    readDrop,
    sortEntries,
    takeDrop,
    type Transfer,
  } from '../state/files.svelte';
  import { sessions, type Tab } from '../state/sessions.svelte';
  import { toasts } from '../state/toasts.svelte';
  import type { DroppedFiles, EditInfo, FilesSort, PickedUpload, SftpEntry } from '../types';
  import Icon from './Icon.svelte';

  interface Props {
    tab: Tab;
  }
  let { tab }: Props = $props();

  const uid = $props.id();
  const ROW_HEIGHT = 28;
  /** Typed letters jump to a name; a pause this long starts a new search. */
  const TYPE_AHEAD_MS = 1000;
  const COLUMNS: { id: FilesSort; label: MessageKey }[] = [
    { id: 'name', label: 'files.name' },
    { id: 'size', label: 'files.size' },
    { id: 'modified', label: 'files.modified' },
  ];

  let list = $state<HTMLDivElement>();
  let pane = $state<HTMLDivElement>();
  let pathInput = $state('');

  const connected = $derived(tab.status === 'connected' && tab.sessionId != null);
  const view = $derived(files.views[tab.key]);
  const entries = $derived(
    sortEntries(
      (view?.entries ?? []).filter((e) => app.settings.filesShowHidden || !e.name.startsWith('.')),
      app.settings.filesSort,
      app.settings.filesSortDesc,
    ),
  );
  const selected = $derived(new Set(view?.selection ?? []));
  /** Files of this tab's session open in the editor. */
  const myEdits = $derived(edits.forSession(tab.sessionId));
  /** The selected entries that are shown, in list order: what commands act on. */
  const chosen = $derived(entries.filter((e) => selected.has(e.name)));
  const chosenSize = $derived(chosen.reduce((sum, e) => sum + (isDirLike(e) ? 0 : e.size), 0));
  const cursorIndex = $derived(view?.cursor == null ? -1 : entries.findIndex((e) => e.name === view.cursor));
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

  // ---- selection ---------------------------------------------------------------

  function indexOf(name: string | null): number {
    return name === null ? -1 : entries.findIndex((e) => e.name === name);
  }

  function selectOnly(name: string | null) {
    if (!view) return;
    view.selection = name === null ? [] : [name];
    view.cursor = name;
    view.anchor = name;
  }

  function toggle(name: string) {
    if (!view) return;
    view.selection = selected.has(name) ? view.selection.filter((n) => n !== name) : [...view.selection, name];
    view.cursor = name;
    view.anchor = name;
  }

  /** Selects from the anchor to `name`; `add` keeps what was selected already. */
  function selectRange(name: string, add: boolean) {
    if (!view) return;
    const to = indexOf(name);
    if (to < 0) return;
    let from = indexOf(view.anchor);
    if (from < 0) {
      from = to;
      view.anchor = name;
    }
    const range = entries.slice(Math.min(from, to), Math.max(from, to) + 1).map((e) => e.name);
    view.selection = add ? [...new Set([...view.selection, ...range])] : range;
    view.cursor = name;
  }

  function selectAll() {
    if (!view || !entries.length) return;
    view.selection = entries.map((e) => e.name);
    view.cursor ??= entries[0].name;
    view.anchor ??= view.cursor;
  }

  function scrollTo(name: string) {
    list?.querySelector<HTMLElement>(`[data-name="${CSS.escape(name)}"]`)?.scrollIntoView({ block: 'nearest' });
  }

  function onRowClick(ev: MouseEvent, e: SftpEntry) {
    if (ev.shiftKey) selectRange(e.name, mod(ev));
    else if (mod(ev)) toggle(e.name);
    else selectOnly(e.name);
  }

  function onListClick(ev: MouseEvent) {
    const target = ev.target as Element;
    if (!target.closest('.row, .columns') && !ev.shiftKey && !mod(ev)) selectOnly(null);
  }

  // ---- commands ----------------------------------------------------------------

  function activate(e: SftpEntry) {
    if (!view?.path) return;
    if (isDirLike(e)) void files.openDir(tab, joinPath(view.path, e.name));
    else if (app.settings.filesDoubleClick === 'edit') edit(e);
    else void files.download(tab, [e]);
  }

  /** Opens a file in the editor; saves go back to the server. */
  function edit(e: SftpEntry) {
    if (!view?.path || isDirLike(e)) return;
    void edits.open(tab, joinPath(view.path, e.name));
  }

  function go(ev: SubmitEvent) {
    ev.preventDefault();
    const path = pathInput.trim();
    if (path) void files.goTo(tab, path);
  }

  async function copyPath(path: string) {
    await navigator.clipboard.writeText(path).catch(() => {});
    toasts.show(t('common.copied', { text: path }));
  }

  function entryMenu(ev: MouseEvent, e: SftpEntry) {
    ev.preventDefault();
    ev.stopPropagation();
    if (!view?.path) return;
    // A right-click inside the selection acts on all of it; elsewhere it selects that entry.
    if (selected.has(e.name)) view.cursor = e.name;
    else selectOnly(e.name);
    const targets = chosen;
    const items: MenuEntry[] = [];
    if (targets.length > 1) {
      items.push(
        { label: t('files.download'), icon: 'download', action: () => void files.download(tab, targets) },
        { label: t('files.downloadTo'), action: () => void files.downloadTo(tab, targets) },
        'separator',
        { label: t('files.permissions'), icon: 'lock', action: () => void files.chmod(tab, targets) },
        'separator',
        { label: t('common.delete'), icon: 'trash', danger: true, hint: 'Del', action: () => void files.remove(tab, targets) },
      );
    } else {
      const path = joinPath(view.path, e.name);
      if (isDirLike(e)) {
        items.push(
          { label: t('files.openItem'), icon: 'folder', action: () => activate(e) },
          { label: t('files.openInTerminal'), icon: 'terminal', action: () => files.openInTerminal(tab, path) },
          'separator',
        );
      }
      if (!isDirLike(e)) {
        items.push(
          { label: t('files.editIn', { editor: edits.editorName(e.name) }), icon: 'external', hint: 'F4', action: () => edit(e) },
          'separator',
        );
      }
      items.push(
        { label: t('files.download'), icon: 'download', action: () => void files.download(tab, [e]) },
        { label: t('files.downloadTo'), action: () => void files.downloadTo(tab, [e]) },
        'separator',
        { label: t('files.rename'), icon: 'edit', hint: 'F2', action: () => void files.rename(tab, e) },
        { label: t('files.permissions'), icon: 'lock', action: () => void files.chmod(tab, [e]) },
        { label: t('files.copyPath'), icon: 'copy', action: () => copyPath(path) },
        'separator',
        { label: t('common.delete'), icon: 'trash', danger: true, hint: 'Del', action: () => void files.remove(tab, [e]) },
      );
    }
    app.showMenu(ev.clientX, ev.clientY, items);
  }

  function folderMenu(ev: MouseEvent) {
    ev.preventDefault();
    if (!view?.path) return;
    const path = view.path;
    selectOnly(null);
    app.showMenu(ev.clientX, ev.clientY, [
      { label: t('files.uploadFiles'), icon: 'upload', action: () => void files.uploadFiles(tab) },
      { label: t('files.uploadFolder'), icon: 'folder', action: () => void files.uploadFolder(tab) },
      'separator',
      { label: t('files.newFolder'), icon: 'folderPlus', action: () => void files.mkdir(tab) },
      { label: t('files.newFile'), icon: 'file', action: () => void files.newFile(tab) },
      'separator',
      { label: t('files.openInTerminal'), icon: 'terminal', action: () => files.openInTerminal(tab, path) },
      { label: t('files.selectAll'), hint: shortcut('Mod', 'A'), disabled: !entries.length, action: selectAll },
      { label: t('files.refresh'), icon: 'refresh', action: () => files.refresh(tab) },
      'separator',
      {
        label: t('files.showHidden'),
        icon: app.settings.filesShowHidden ? 'check' : undefined,
        action: () => app.update({ filesShowHidden: !app.settings.filesShowHidden }),
      },
    ]);
  }

  function uploadMenu(ev: MouseEvent) {
    const button = (ev.currentTarget as HTMLElement).getBoundingClientRect();
    app.showMenu(button.left, button.bottom + 4, [
      { label: t('files.uploadFiles'), icon: 'file', action: () => void files.uploadFiles(tab) },
      { label: t('files.uploadFolder'), icon: 'folder', action: () => void files.uploadFolder(tab) },
    ]);
  }

  function sortBy(id: FilesSort) {
    if (app.settings.filesSort === id) app.update({ filesSortDesc: !app.settings.filesSortDesc });
    // Biggest and newest first; names from A.
    else app.update({ filesSort: id, filesSortDesc: id !== 'name' });
  }

  // ---- keyboard ----------------------------------------------------------------

  let typed = '';
  let typedAt = 0;

  /** Jumps to the next name starting with the letters typed just now. */
  function typeAhead(ch: string) {
    if (!view || !entries.length) return;
    const now = Date.now();
    typed = now - typedAt > TYPE_AHEAD_MS ? ch : typed + ch;
    typedAt = now;
    const text = typed.toLocaleLowerCase();
    // The same letter again cycles through the names that start with it.
    const cycle = [...text].every((c) => c === text[0]);
    const prefix = cycle ? text[0] : text;
    const start = Math.max(0, indexOf(view.cursor) + (cycle ? 1 : 0));
    for (let i = 0; i < entries.length; i++) {
      const e = entries[(start + i) % entries.length];
      if (e.name.toLocaleLowerCase().startsWith(prefix)) {
        selectOnly(e.name);
        scrollTo(e.name);
        return;
      }
    }
  }

  function onkeydown(ev: KeyboardEvent) {
    if (ev.key === 'Escape') {
      ev.preventDefault();
      sessions.focusActive();
      return;
    }
    // Keys on the buttons inside the list (sorting, Retry) are theirs.
    if (!view?.path || (ev.target as Element).closest('button')) return;
    const withMod = mod(ev);
    const index = indexOf(view.cursor);
    const current = index >= 0 ? entries[index] : undefined;
    const page = Math.max(1, Math.floor((list?.clientHeight ?? 0) / ROW_HEIGHT) - 1);
    const moveTo = (to: number) => {
      if (!entries.length) return;
      const next = entries[Math.max(0, Math.min(entries.length - 1, to))];
      if (ev.shiftKey) selectRange(next.name, withMod);
      else if (withMod) view.cursor = next.name; // moves without selecting; Space selects
      else selectOnly(next.name);
      scrollTo(next.name);
    };
    switch (ev.key) {
      case 'ArrowDown':
        moveTo(index + 1);
        break;
      case 'ArrowUp':
        moveTo(index < 0 ? 0 : index - 1);
        break;
      case 'PageDown':
        moveTo(index + page);
        break;
      case 'PageUp':
        moveTo(index - page);
        break;
      case 'Home':
        moveTo(0);
        break;
      case 'End':
        moveTo(entries.length - 1);
        break;
      case 'Enter':
        if (current) activate(current);
        break;
      case 'Backspace':
        files.up(tab);
        break;
      case 'Delete':
        if (chosen.length) void files.remove(tab, chosen);
        break;
      case 'F2':
        if (current) void files.rename(tab, current);
        break;
      case 'F4':
        if (current) edit(current);
        break;
      case ' ':
        if (!withMod && Date.now() - typedAt < TYPE_AHEAD_MS) typeAhead(' ');
        else if (current) toggle(current.name);
        break;
      default:
        if (withMod && !ev.shiftKey && !ev.altKey && ev.key.toLowerCase() === 'a') selectAll();
        else if (ev.key.length === 1 && !withMod && !ev.altKey && !ev.ctrlKey && !ev.metaKey) typeAhead(ev.key);
        else return;
    }
    ev.preventDefault();
  }

  // ---- drag & drop -------------------------------------------------------------

  let dragging = $state(false);
  /** The folder row under the pointer while dragging: the drop goes into it. */
  let dropFolder = $state<string | null>(null);
  /** dragenter/dragleave pairs of the elements inside: 0 means the drag left. */
  let dragDepth = 0;
  /** Linux: what the native handler says is being dragged in. WebKitGTK shows the page such
   * a drag as a link without the files, so a drop here uploads these by path. */
  let nativeItems: PickedUpload[] = [];

  $effect(() => {
    if (currentOs() !== 'linux') return;
    const stops = [
      listen<PickedUpload[]>('files-dragged', ({ payload }) => (nativeItems = payload)),
      // When the native handler takes the drop itself, the page gets no drop event.
      listen<DroppedFiles>('files-dropped', ({ payload }) => nativeDrop(payload)),
    ];
    return () => stops.forEach((stop) => void stop.then((unlisten) => unlisten()));
  });

  function isFileDrag(ev: DragEvent) {
    const types = ev.dataTransfer?.types ?? [];
    return types.includes('Files') || (nativeItems.length > 0 && types.includes('text/uri-list'));
  }

  function folderAt(target: EventTarget | null): string | null {
    const name = (target as Element | null)?.closest?.<HTMLElement>('.row')?.dataset.name;
    const entry = name === undefined ? undefined : entries.find((e) => e.name === name);
    return entry && isDirLike(entry) ? entry.name : null;
  }

  function endDrag() {
    dragDepth = 0;
    dragging = false;
    dropFolder = null;
  }

  function ondragenter(ev: DragEvent) {
    if (!isFileDrag(ev) || !view?.path) return;
    dragDepth++;
    dragging = true;
  }

  function ondragleave() {
    if (dragDepth > 0 && --dragDepth === 0) endDrag();
  }

  function ondragover(ev: DragEvent) {
    if (!isFileDrag(ev) || !view?.path) return;
    ev.preventDefault();
    if (ev.dataTransfer) ev.dataTransfer.dropEffect = 'copy';
    dragging = true;
    dropFolder = folderAt(ev.target);
  }

  function ondrop(ev: DragEvent) {
    const folder = dropFolder;
    endDrag();
    if (!isFileDrag(ev) || !ev.dataTransfer || !view?.path) return;
    ev.preventDefault();
    const dir = folder ? joinPath(view.path, folder) : undefined;
    if (!ev.dataTransfer.types.includes('Files')) {
      const items = nativeItems;
      nativeItems = [];
      void files.uploadPicked(tab, items, dir);
      return;
    }
    // Uploaded from disk where the app can tell the files' paths, else sent from the page.
    const dropped = takeDrop(ev.dataTransfer);
    pathsOfDrop(dropped)
      .then((picked) =>
        picked ? files.uploadPicked(tab, picked, dir) : readDrop(dropped).then((items) => files.upload(tab, items, dir)),
      )
      .catch((e) => toasts.error(errorMessage(e)));
  }

  function nativeDrop({ items, x, y }: DroppedFiles) {
    nativeItems = [];
    const element = document.elementFromPoint(x, y);
    if (!element || !pane?.contains(element) || !view?.path) return;
    const folder = folderAt(element);
    void files.uploadPicked(tab, items, folder ? joinPath(view.path, folder) : undefined);
  }

  // ---- edited files ------------------------------------------------------------

  function editState(e: EditInfo): string {
    switch (e.state) {
      case 'synced':
        return e.savedAt
          ? t('edit.savedAt', {
              time: new Date(e.savedAt).toLocaleTimeString(i18n.lang, { hour: '2-digit', minute: '2-digit' }),
            })
          : t('edit.opened');
      case 'uploading':
        return t('edit.uploading');
      case 'conflict':
        return t('edit.conflict');
      case 'denied':
        return t(e.needsPassword ? 'edit.needsPassword' : 'edit.denied');
      case 'waiting':
        return t('edit.waiting');
      case 'failed':
        return t('edit.failed');
    }
  }

  // ---- transfers ---------------------------------------------------------------

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

  /** Speed and time left, once measured. */
  function pace(tr: Transfer) {
    if (!tr.rate) return '';
    const speed = t('files.speed', { size: formatSize(tr.rate) });
    if (!tr.total || tr.done >= tr.total) return speed;
    return `${speed} · ${t('files.left', { time: formatDuration((tr.total - tr.done) / tr.rate) })}`;
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

<aside class="files" style:width="{width}px" aria-label={t('files.title')} data-keep-focus>
  <!-- svelte-ignore a11y_no_static_element_interactions -->
  <div class="resizer" class:resizing onpointerdown={startResize} ondblclick={() => app.update({ filesWidth: 380 })}></div>

  <header>
    <div class="top">
      <span class="title">{t('files.title')}</span>
      <span class="host" title={tab.subtitle}>{tab.title}</span>
      <span class="spacer"></span>
      <button
        class="icon-btn small"
        title={t('files.upload')}
        aria-label={t('files.upload')}
        aria-haspopup="menu"
        disabled={!view?.path}
        onclick={uploadMenu}
      >
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

  <!-- svelte-ignore a11y_no_static_element_interactions -->
  <div class="pane" class:over={dragging && !dropFolder} bind:this={pane} {ondragenter} {ondragleave} {ondragover} {ondrop}>
    <!-- svelte-ignore a11y_no_noninteractive_tabindex -->
    <div
      class="list"
      bind:this={list}
      tabindex="0"
      role="listbox"
      aria-multiselectable="true"
      aria-label={t('files.title')}
      aria-activedescendant={cursorIndex >= 0 ? `${uid}-${cursorIndex}` : undefined}
      {onkeydown}
      onclick={onListClick}
      oncontextmenu={(e) => connected && view?.path && folderMenu(e)}
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
        <div class="columns" role="group">
          {#each COLUMNS as col (col.id)}
            <!-- Sorting leaves the focus where it was (the list, usually). -->
            <button
              class="col by-{col.id}"
              class:sorted={app.settings.filesSort === col.id}
              onmousedown={(e) => e.preventDefault()}
              onclick={() => sortBy(col.id)}
            >
              <span>{t(col.label)}</span>
              {#if app.settings.filesSort === col.id}
                <span class="arrow" class:up={!app.settings.filesSortDesc}><Icon name="chevronDown" size={12} /></span>
              {/if}
            </button>
          {/each}
        </div>
        {#each entries as e, i (e.name)}
          <!-- The list keeps the focus (rows would lose it when the folder changes) and moves
               through the rows itself: aria-activedescendant. -->
          <!-- svelte-ignore a11y_interactive_supports_focus, a11y_click_events_have_key_events -->
          <div
            id="{uid}-{i}"
            class="row"
            class:selected={selected.has(e.name)}
            class:cursor={view.cursor === e.name && (chosen.length > 1 || !selected.has(e.name))}
            class:target={dropFolder === e.name}
            data-name={e.name}
            role="option"
            aria-selected={selected.has(e.name)}
            title={e.permissions ? `${e.name}\n${e.permissions}` : e.name}
            onclick={(ev) => onRowClick(ev, e)}
            ondblclick={() => activate(e)}
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
    </div>
    {#if dragging}
      <div class="drop">
        <Icon name="upload" size={14} />
        <span>{t('files.drop', { dir: dropFolder ?? dirName })}</span>
      </div>
    {/if}
  </div>

  {#if chosen.length > 1}
    <div class="summary">
      {tn('files.selected', chosen.length)}{chosenSize ? ` · ${formatSize(chosenSize)}` : ''}
    </div>
  {/if}

  {#if myEdits.length}
    <footer>
      <div class="thead"><span>{t('edit.title')}</span></div>
      <div class="tlist">
        {#each myEdits as edit (edit.id)}
          {@const asks = edit.state === 'conflict' || edit.state === 'denied'}
          <div
            class="transfer edit"
            class:failed={edit.state === 'failed'}
            title={edit.error ? `${edit.remotePath} — ${edit.error}` : edit.remotePath}
          >
            <span class="ticon"><Icon name={edit.sudo ? 'shield' : 'external'} size={13} /></span>
            <span class="tname">{edit.name}</span>
            {#if asks}
              <button class="link ask" onclick={() => edits.resolve(edit)}>{editState(edit)}</button>
            {:else}
              <span class="tstate">{editState(edit)}</span>
            {/if}
            <span class="eactions">
              <button
                class="icon-btn small"
                title={t('edit.openAgain', { editor: edits.editorName(edit.name) })}
                aria-label={t('edit.openAgain', { editor: edits.editorName(edit.name) })}
                onclick={() => edits.show(edit)}
              >
                <Icon name="external" size={12} />
              </button>
              <button class="icon-btn small" title={t('edit.reveal')} aria-label={t('edit.reveal')} onclick={() => edits.reveal(edit)}>
                <Icon name="folder" size={12} />
              </button>
              <button class="icon-btn small" title={t('edit.stop')} aria-label={t('edit.stop')} onclick={() => edits.stop(edit)}>
                <Icon name="x" size={12} />
              </button>
            </span>
            {#if edit.state === 'failed' && edit.error}
              <span class="terror">{edit.error}</span>
            {/if}
          </div>
        {/each}
      </div>
    </footer>
  {/if}

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
              <span class="tpace">{pace(tr)}</span>
            {:else if tr.status === 'failed' && tr.error}
              <span class="terror">{tr.error}</span>
            {/if}
          </div>
        {/each}
      </div>
    </footer>
  {/if}

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
    container-type: inline-size;
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
  .pane {
    position: relative;
    flex: 1;
    min-height: 0;
    display: flex;
    flex-direction: column;
  }
  .pane.over::after {
    content: '';
    position: absolute;
    inset: 4px;
    border: 1.5px dashed color-mix(in srgb, var(--green) 65%, transparent);
    border-radius: var(--radius);
    background: color-mix(in srgb, var(--green) 6%, transparent);
    pointer-events: none;
  }
  .list {
    flex: 1;
    min-height: 0;
    overflow: auto;
    padding: 0 4px 4px;
    outline: none;
  }
  /* The header and the rows share their columns; without subgrid, the columns are fixed. */
  .columns,
  .row {
    display: grid;
    grid-template-columns: 18px minmax(0, 1fr) 62px 112px;
    align-items: center;
    gap: 8px;
    padding: 0 8px;
  }
  @supports (grid-template-columns: subgrid) {
    /* The first column includes the rows' padding, which subgrid adds to its edge items. */
    .list {
      display: grid;
      grid-template-columns: 26px minmax(0, 1fr) auto auto;
      align-content: start;
      column-gap: 8px;
    }
    .list > :global(*) {
      grid-column: 1 / -1;
    }
    .columns,
    .row {
      grid-template-columns: subgrid;
    }
  }
  .columns {
    position: sticky;
    top: 0;
    z-index: 1;
    height: 26px;
    margin-bottom: 2px;
    border-bottom: 1px solid var(--border-soft);
    background: var(--bg);
  }
  .col {
    display: flex;
    align-items: center;
    gap: 2px;
    min-width: 0;
    height: 100%;
    padding: 0;
    border: 0;
    background: none;
    color: var(--text-3);
    font: inherit;
    font-size: 11px;
    font-weight: 600;
    white-space: nowrap;
  }
  .col span {
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .col.by-name {
    grid-column: 1 / 3;
  }
  .col.by-size,
  .col.by-modified {
    justify-content: flex-end;
  }
  .col:hover,
  .col.sorted {
    color: var(--text-2);
  }
  .arrow {
    display: grid;
    flex: none;
  }
  .arrow.up {
    transform: rotate(180deg);
  }
  .row {
    height: 28px;
    border-radius: var(--radius-sm);
    cursor: default;
    user-select: none;
    /* Scrolled to with the keyboard, a row stays clear of the sticky header. */
    scroll-margin-top: 28px;
  }
  .row:hover {
    background: var(--hover);
  }
  .row.selected {
    background: var(--active);
  }
  .list:focus-within .row.cursor {
    box-shadow: inset 0 0 0 1px color-mix(in srgb, var(--text-2) 55%, transparent);
  }
  .row.target {
    background: color-mix(in srgb, var(--green) 16%, transparent);
    box-shadow: inset 0 0 0 1.5px color-mix(in srgb, var(--green) 70%, transparent);
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
    overflow: hidden;
    color: var(--text-2);
    font-size: 11.5px;
    white-space: nowrap;
    text-overflow: ellipsis;
    text-align: right;
    font-variant-numeric: tabular-nums;
  }
  /* A narrow drawer leaves the date out. */
  @container (max-width: 340px) {
    .columns,
    .row {
      grid-template-columns: 18px minmax(0, 1fr) 62px;
    }
    .list {
      grid-template-columns: 26px minmax(0, 1fr) auto;
    }
    @supports (grid-template-columns: subgrid) {
      .columns,
      .row {
        grid-template-columns: subgrid;
      }
    }
    .date,
    .col.by-modified {
      display: none;
    }
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
    left: 50%;
    bottom: 14px;
    z-index: 2;
    display: flex;
    align-items: center;
    gap: 6px;
    max-width: calc(100% - 32px);
    padding: 6px 12px;
    border-radius: 999px;
    background: var(--text);
    color: var(--bg);
    font-size: 12px;
    box-shadow: var(--shadow);
    transform: translateX(-50%);
    pointer-events: none;
  }
  .drop span {
    overflow: hidden;
    white-space: nowrap;
    text-overflow: ellipsis;
  }
  .summary {
    padding: 5px 12px;
    border-top: 1px solid var(--border-soft);
    color: var(--text-2);
    font-size: 11.5px;
    font-variant-numeric: tabular-nums;
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
  .transfer.edit {
    grid-template-columns: 16px minmax(0, 1fr) auto auto;
  }
  .eactions {
    display: flex;
  }
  .link.ask {
    color: var(--amber);
    font-size: 11.5px;
    text-decoration: underline;
    text-underline-offset: 2px;
  }
  .link.ask:hover {
    color: var(--text);
  }
  .tbar {
    grid-column: 2;
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
  .tpace {
    grid-column: 3 / 5;
    color: var(--text-3);
    font-size: 11px;
    white-space: nowrap;
    text-align: right;
    font-variant-numeric: tabular-nums;
  }
  .terror {
    grid-column: 2 / 5;
    overflow: hidden;
    color: var(--red);
    font-size: 11px;
    white-space: nowrap;
    text-overflow: ellipsis;
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
