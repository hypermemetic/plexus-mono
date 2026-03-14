import { PlexusRpcClient } from '../generated/transport';
import { createPlayerClient } from '../generated/player/client';
import { createPlayerPlaylistClient } from '../generated/player/playlist/client';
import { createMonoClient } from '../generated/mono/client';
import type { MonoEvent, MonoEventNowPlaying, MonoEventCover, MonoEventPlaylistInfo, MonoEventSearchTrack, MonoEventQueue, QueuedTrack } from '../generated/player/types';
import { PlexusRpcClient as SubstratePlexusRpcClient } from '../generated-substrate/transport';
import { createClaudecodeClient } from '../generated-substrate/claudecode/client';
import type { ChatEvent } from '../generated-substrate/claudecode/types';
import { isPermissionGranted, requestPermission, sendNotification } from '@tauri-apps/plugin-notification';

// --- Show/hide animations + click-away dismiss ---
import { listen } from '@tauri-apps/api/event';
import { getCurrentWindow, LogicalSize } from '@tauri-apps/api/window';

const appEl = document.getElementById('app')!;
const appWindow = getCurrentWindow();
let hiding = false;

type ViewName = 'now-playing' | 'browse' | 'detail' | 'queue' | 'research';

const VIEW_HEIGHTS: Record<ViewName, number> = {
  'now-playing': 556,
  'browse': 600,
  'detail': 600,
  'queue': 600,
  'research': 650,
};

listen('mono-tray://show', () => {
  hiding = false;
  appEl.classList.remove('animate-out');
  appEl.classList.remove('animate-in');
  void appEl.offsetWidth;
  appEl.classList.add('animate-in');
});

listen('mono-tray://hide', () => {
  if (hiding) return;
  hiding = true;
  // Reset to now-playing before hiding
  navigateTo('now-playing');
  playlistPicker.classList.add('hidden');
  appEl.classList.remove('animate-in');
  appEl.classList.add('animate-out');
  setTimeout(() => {
    appWindow.hide();
    hiding = false;
  }, 130);
});

// --- DOM elements ---
const albumArt = document.getElementById('album-art') as HTMLImageElement;
const titleEl = document.getElementById('title')!;
const artistAlbumEl = document.getElementById('artist-album')!;
const progressFill = document.getElementById('progress-fill')!;
const progressThumb = document.getElementById('progress-thumb')!;
const timeCurrent = document.getElementById('time-current')!;
const timeTotal = document.getElementById('time-total')!;
const btnPlayPause = document.getElementById('btn-play-pause')!;
const iconPlay = document.getElementById('icon-play')!;
const iconPause = document.getElementById('icon-pause')!;
const btnPrevious = document.getElementById('btn-previous')!;
const btnNext = document.getElementById('btn-next')!;
const volumeSlider = document.getElementById('volume-slider') as HTMLInputElement;
const queueBtn = document.getElementById('queue-btn')!;
const queueInfo = document.getElementById('queue-info')!;
const openLink = document.getElementById('open-link') as HTMLAnchorElement;
const disconnectOverlay = document.getElementById('disconnect-overlay')!;

// Nav elements
const navBack = document.getElementById('nav-back')!;
const navTitle = document.getElementById('nav-title')!;
const navAction = document.getElementById('nav-action')!;
const iconList = document.getElementById('icon-list')!;
const iconPlayAll = document.getElementById('icon-play-all')!;
const iconClear = document.getElementById('icon-clear')!;
const viewContainer = document.getElementById('view-container')!;
const searchInput = document.getElementById('search-input') as HTMLInputElement;
const browseList = document.getElementById('browse-list')!;
const detailSubheader = document.getElementById('detail-subheader')!;
const detailTracks = document.getElementById('detail-tracks')!;
const queueSubheader = document.getElementById('queue-subheader')!;
const queueTracksEl = document.getElementById('queue-tracks')!;
const breadcrumbs = document.getElementById('breadcrumbs')!;

// --- State ---
let currentTrackId: number | null = null;
let isPlaying = false;
let volumeDebounce: ReturnType<typeof setTimeout> | null = null;
let notificationsEnabled = false;
let currentView: ViewName = 'now-playing';
let cachedPlaylists: MonoEventPlaylistInfo[] | null = null;
let currentPlaylistName: string | null = null;
let searchDebounce: ReturnType<typeof setTimeout> | null = null;
let activeSearchGen: AsyncGenerator | null = null;
let browseScrollTop = 0;
let lastQueueLength = 0;
let lastEnterTime = 0;
let researchResult: { name: string; tracks: { id: number; title: string; artist: string; reason: string }[] } | null = null;
let isResearching = false;
let pendingAddTrackId: number | null = null;

// New DOM elements
const sparkleBtn = document.getElementById('sparkle-btn')!;
const sparkleBadge = document.getElementById('sparkle-badge')!;
const researchNameInput = document.getElementById('research-name') as HTMLInputElement;
const researchTracksEl = document.getElementById('research-tracks')!;
const researchCreateBtn = document.getElementById('research-create-btn')!;
const researchQueueBtn = document.getElementById('research-queue-btn')!;
const playlistPicker = document.getElementById('playlist-picker')!;
const playlistPickerList = document.getElementById('playlist-picker-list')!;
const playlistPickerNew = document.getElementById('playlist-picker-new')!;

// --- Notifications ---
async function initNotifications(): Promise<void> {
  try {
    let granted = await isPermissionGranted();
    if (!granted) {
      const permission = await requestPermission();
      granted = permission === 'granted';
    }
    notificationsEnabled = granted;
  } catch {
    notificationsEnabled = false;
  }
}

function notifyTrackChange(np: MonoEventNowPlaying): void {
  if (!notificationsEnabled || !np.title) return;
  const body = np.artist ? `${np.artist}${np.album ? ' — ' + np.album : ''}` : '';
  sendNotification({ title: np.title, body });
}

initNotifications();

// --- RPC client ---
const rpc = new PlexusRpcClient({
  backend: 'monochrome',
  url: 'ws://127.0.0.1:4448',
  debug: false,
});

const player = createPlayerClient(rpc);
const playlist = createPlayerPlaylistClient(rpc);
const mono = createMonoClient(rpc);

// Substrate RPC client for AI research (claudecode)
const substrateRpc = new SubstratePlexusRpcClient({
  backend: 'substrate',
  url: 'ws://127.0.0.1:4444',
  debug: false,
});
const claudecode = createClaudecodeClient(substrateRpc);

// Fire-and-forget RPC helper
async function rpcFire(gen: AsyncGenerator): Promise<void> {
  try { for await (const _ of gen) { break; } } catch { /* ignore */ }
}

// --- Helpers ---
function formatTime(secs: number): string {
  const m = Math.floor(secs / 60);
  const s = Math.floor(secs % 60);
  return `${m}:${s.toString().padStart(2, '0')}`;
}

async function fetchCoverArt(trackId: number): Promise<void> {
  try {
    for await (const event of mono.cover(trackId, 640)) {
      if (event.type === 'cover') {
        const cover = event as MonoEventCover;
        albumArt.src = cover.url;
        albumArt.classList.add('loaded');
        return;
      }
    }
  } catch {
    albumArt.classList.remove('loaded');
  }
}

function updatePlayPauseIcon(status: string): void {
  isPlaying = status === 'playing' || status === 'buffering' || status === 'starting';
  iconPlay.classList.toggle('hidden', isPlaying);
  iconPause.classList.toggle('hidden', !isPlaying);
}

function updateUI(np: MonoEventNowPlaying): void {
  titleEl.textContent = np.title || 'Not Playing';

  const parts: string[] = [];
  if (np.artist) parts.push(np.artist);
  if (np.album) parts.push(np.album);
  artistAlbumEl.textContent = parts.join(' — ');

  const pct = np.durationSecs > 0 ? (np.positionSecs / np.durationSecs) * 100 : 0;
  progressFill.style.width = `${pct}%`;
  progressThumb.style.left = `${pct}%`;
  timeCurrent.textContent = formatTime(np.positionSecs);
  timeTotal.textContent = formatTime(np.durationSecs);

  updatePlayPauseIcon(np.status);

  if (!volumeSlider.matches(':active')) {
    volumeSlider.value = String(Math.round(np.volume * 100));
  }

  lastQueueLength = np.queueLength;
  if (np.queueLength > 0) {
    queueInfo.textContent = `${np.queueLength} in queue`;
  } else {
    queueInfo.textContent = '';
  }

  if (np.trackId) {
    openLink.style.display = '';
    openLink.dataset.url = `https://monochrome.tf/track/t/${np.trackId}`;
  } else {
    openLink.style.display = 'none';
  }

  if (np.trackId && np.trackId !== currentTrackId) {
    currentTrackId = np.trackId;
    fetchCoverArt(np.trackId);
    notifyTrackChange(np);
  } else if (!np.trackId) {
    currentTrackId = null;
    albumArt.classList.remove('loaded');
  }
}

// --- Navigation ---
function updateBreadcrumbs(view: ViewName): void {
  breadcrumbs.innerHTML = '';
  if (view === 'now-playing') return;

  type Crumb = { label: string; view: ViewName } | { label: string; current: true };
  const trail: Crumb[] = [];

  if (view === 'browse') {
    trail.push({ label: 'Now Playing', view: 'now-playing' });
    trail.push({ label: 'Library', current: true });
  } else if (view === 'detail') {
    trail.push({ label: 'Now Playing', view: 'now-playing' });
    trail.push({ label: 'Library', view: 'browse' });
    trail.push({ label: currentPlaylistName || 'Playlist', current: true });
  } else if (view === 'queue') {
    trail.push({ label: 'Now Playing', view: 'now-playing' });
    trail.push({ label: 'Queue', current: true });
  } else if (view === 'research') {
    trail.push({ label: 'Now Playing', view: 'now-playing' });
    trail.push({ label: 'Library', view: 'browse' });
    trail.push({ label: 'Research', current: true });
  }

  trail.forEach((crumb, i) => {
    if (i > 0) {
      const sep = document.createElement('span');
      sep.className = 'crumb-sep';
      sep.textContent = '›';
      breadcrumbs.appendChild(sep);
    }
    const el = document.createElement('span');
    if ('current' in crumb) {
      el.className = 'crumb-current';
      el.textContent = crumb.label;
    } else {
      el.className = 'crumb';
      el.textContent = crumb.label;
      const target = crumb.view;
      el.addEventListener('click', () => {
        navigateTo(target);
        if (target === 'browse') loadPlaylists();
      });
    }
    breadcrumbs.appendChild(el);
  });
}

function navigateTo(view: ViewName): void {
  // Save browse scroll before leaving
  if (currentView === 'browse' && view !== 'browse') {
    browseScrollTop = browseList.scrollTop;
  }

  // Clear search when leaving browse
  if (currentView === 'browse' && view === 'now-playing') {
    searchInput.value = '';
    if (activeSearchGen) {
      activeSearchGen.return(undefined);
      activeSearchGen = null;
    }
  }

  currentView = view;
  viewContainer.dataset.view = view;

  // Resize window to fit view
  appWindow.setSize(new LogicalSize(352, VIEW_HEIGHTS[view]));

  // Update breadcrumbs
  updateBreadcrumbs(view);

  // Update nav header
  switch (view) {
    case 'now-playing':
      navBack.classList.add('hidden');
      navTitle.textContent = '';
      iconList.classList.remove('hidden');
      iconPlayAll.classList.add('hidden');
      iconClear.classList.add('hidden');
      navAction.classList.remove('hidden');
      navAction.title = 'Library';
      break;
    case 'browse':
      navBack.classList.remove('hidden');
      navTitle.textContent = 'Library';
      iconList.classList.add('hidden');
      iconPlayAll.classList.add('hidden');
      iconClear.classList.add('hidden');
      navAction.classList.add('hidden');
      requestAnimationFrame(() => { browseList.scrollTop = browseScrollTop; });
      break;
    case 'detail':
      navBack.classList.remove('hidden');
      navTitle.textContent = currentPlaylistName || 'Playlist';
      iconList.classList.add('hidden');
      iconPlayAll.classList.remove('hidden');
      iconClear.classList.add('hidden');
      navAction.classList.remove('hidden');
      navAction.title = 'Play All';
      break;
    case 'queue':
      navBack.classList.remove('hidden');
      navTitle.textContent = 'Queue';
      iconList.classList.add('hidden');
      iconPlayAll.classList.add('hidden');
      iconClear.classList.remove('hidden');
      navAction.classList.remove('hidden');
      navAction.title = 'Clear queue';
      break;
    case 'research':
      navBack.classList.remove('hidden');
      navTitle.textContent = 'Research';
      iconList.classList.add('hidden');
      iconPlayAll.classList.add('hidden');
      iconClear.classList.add('hidden');
      navAction.classList.add('hidden');
      break;
  }
}

// --- Playlist loading ---
async function loadPlaylists(): Promise<void> {
  if (cachedPlaylists) {
    renderPlaylistList(cachedPlaylists);
    return;
  }
  browseList.innerHTML = '';
  const emptyEl = document.createElement('div');
  emptyEl.className = 'list-empty';
  emptyEl.textContent = 'Loading...';
  browseList.appendChild(emptyEl);

  try {
    const playlists: MonoEventPlaylistInfo[] = [];
    for await (const event of playlist.list()) {
      if (event.type === 'playlist_info') {
        playlists.push(event as MonoEventPlaylistInfo);
      }
    }
    cachedPlaylists = playlists;
    renderPlaylistList(playlists);
  } catch {
    browseList.innerHTML = '';
    const errEl = document.createElement('div');
    errEl.className = 'list-empty';
    errEl.textContent = 'Failed to load playlists';
    browseList.appendChild(errEl);
  }
}

function renderPlaylistList(playlists: MonoEventPlaylistInfo[]): void {
  browseList.innerHTML = '';

  // New Playlist button at top
  const newRow = document.createElement('div');
  newRow.className = 'new-playlist-row';
  newRow.innerHTML = '<svg width="14" height="14" viewBox="0 0 24 24" fill="currentColor"><path d="M19 13h-6v6h-2v-6H5v-2h6V5h2v6h6v2z"/></svg> New Playlist';
  newRow.addEventListener('click', promptCreatePlaylist);
  browseList.appendChild(newRow);

  if (playlists.length === 0) {
    const emptyEl = document.createElement('div');
    emptyEl.className = 'list-empty';
    emptyEl.textContent = 'No playlists';
    browseList.appendChild(emptyEl);
    return;
  }
  for (const pl of playlists) {
    browseList.appendChild(makePlaylistRow(pl));
  }
}

// --- Search ---
function performSearch(query: string): void {
  if (activeSearchGen) {
    activeSearchGen.return(undefined);
    activeSearchGen = null;
  }

  if (!query.trim()) {
    if (cachedPlaylists) {
      renderPlaylistList(cachedPlaylists);
    } else {
      loadPlaylists();
    }
    return;
  }

  const q = query.toLowerCase();

  const matchingPlaylists = (cachedPlaylists || []).filter(
    pl => pl.name.toLowerCase().includes(q)
  );

  const gen = mono.search(query, 'tracks', 8);
  activeSearchGen = gen;

  browseList.innerHTML = '';
  if (matchingPlaylists.length > 0) {
    const header = document.createElement('div');
    header.className = 'list-empty';
    header.textContent = 'Playlists';
    header.style.paddingBottom = '4px';
    browseList.appendChild(header);
    for (const pl of matchingPlaylists) {
      browseList.appendChild(makePlaylistRow(pl));
    }
  }

  (async () => {
    const results: MonoEventSearchTrack[] = [];
    try {
      for await (const event of gen) {
        if (event.type === 'search_track') {
          results.push(event as MonoEventSearchTrack);
        }
      }
    } catch {
      // Search cancelled or failed
    }
    if (activeSearchGen === gen) {
      renderSearchResults(results, matchingPlaylists.length > 0);
      activeSearchGen = null;
    }
  })();
}

function makePlaylistRow(pl: MonoEventPlaylistInfo): HTMLElement {
  const row = document.createElement('div');
  row.className = 'list-row';
  row.addEventListener('click', () => {
    currentPlaylistName = pl.name;
    navigateTo('detail');
    loadPlaylistDetail(pl.name);
  });

  const info = document.createElement('div');
  info.className = 'list-row-info';

  const titleSpan = document.createElement('div');
  titleSpan.className = 'list-row-title';
  titleSpan.textContent = pl.name;

  const sub = document.createElement('div');
  sub.className = 'list-row-sub';
  sub.textContent = `${pl.trackCount} track${pl.trackCount !== 1 ? 's' : ''}`;

  info.appendChild(titleSpan);
  info.appendChild(sub);

  // Delete button
  const deleteBtn = document.createElement('button');
  deleteBtn.className = 'row-action';
  deleteBtn.title = 'Delete playlist';
  deleteBtn.innerHTML = '<svg width="14" height="14" viewBox="0 0 24 24" fill="currentColor"><path d="M6 19c0 1.1.9 2 2 2h8c1.1 0 2-.9 2-2V7H6v12zM19 4h-3.5l-1-1h-5l-1 1H5v2h14V4z"/></svg>';
  deleteBtn.addEventListener('click', (e) => {
    e.stopPropagation();
    if (confirm(`Delete playlist "${pl.name}"?`)) {
      rpcFire(playlist.delete(pl.name));
      cachedPlaylists = cachedPlaylists?.filter(p => p.name !== pl.name) || null;
      if (cachedPlaylists) renderPlaylistList(cachedPlaylists);
    }
  });

  const chevron = document.createElement('span');
  chevron.className = 'list-row-chevron';
  chevron.innerHTML = '<svg width="14" height="14" viewBox="0 0 24 24" fill="currentColor"><path d="M10 6L8.59 7.41 13.17 12l-4.58 4.59L10 18l6-6z"/></svg>';

  row.appendChild(info);
  row.appendChild(deleteBtn);
  row.appendChild(chevron);
  return row;
}

function renderSearchResults(results: MonoEventSearchTrack[], hasPlaylistSection: boolean): void {
  if (!hasPlaylistSection) browseList.innerHTML = '';
  if (results.length === 0 && !hasPlaylistSection) {
    const emptyEl = document.createElement('div');
    emptyEl.className = 'list-empty';
    emptyEl.textContent = 'No results';
    browseList.appendChild(emptyEl);
    return;
  }
  if (results.length === 0) return;
  if (hasPlaylistSection) {
    const header = document.createElement('div');
    header.className = 'list-empty';
    header.textContent = 'Tracks';
    header.style.paddingBottom = '4px';
    browseList.appendChild(header);
  }
  for (const track of results) {
    const row = document.createElement('div');
    row.className = 'list-row';

    const info = document.createElement('div');
    info.className = 'list-row-info';

    const titleSpan = document.createElement('div');
    titleSpan.className = 'list-row-title';
    titleSpan.textContent = track.title;

    const sub = document.createElement('div');
    sub.className = 'list-row-sub';
    sub.textContent = `${track.artist} · ${formatTime(track.durationSecs)}`;

    info.appendChild(titleSpan);
    info.appendChild(sub);

    // Play button
    const playBtn = document.createElement('button');
    playBtn.className = 'row-action';
    playBtn.title = 'Play';
    playBtn.innerHTML = '<svg width="16" height="16" viewBox="0 0 24 24" fill="currentColor"><path d="M8 5v14l11-7z"/></svg>';
    playBtn.addEventListener('click', (e) => {
      e.stopPropagation();
      rpcFire(player.play(track.id));
      navigateTo('now-playing');
    });

    // Queue add button
    const addBtn = document.createElement('button');
    addBtn.className = 'row-action';
    addBtn.title = 'Add to queue';
    addBtn.innerHTML = '<svg width="16" height="16" viewBox="0 0 24 24" fill="currentColor"><path d="M19 13h-6v6h-2v-6H5v-2h6V5h2v6h6v2z"/></svg>';
    addBtn.addEventListener('click', (e) => {
      e.stopPropagation();
      rpcFire(player.queueAdd(track.id));
    });

    // Add to playlist button
    const plBtn = document.createElement('button');
    plBtn.className = 'row-action';
    plBtn.title = 'Add to playlist';
    plBtn.innerHTML = '<svg width="14" height="14" viewBox="0 0 24 24" fill="currentColor"><path d="M14 10H2v2h12v-2zm0-4H2v2h12V6zm4 8v-4h-2v4h-4v2h4v4h2v-4h4v-2h-4zM2 16h8v-2H2v2z"/></svg>';
    plBtn.addEventListener('click', (e) => {
      e.stopPropagation();
      showPlaylistPicker(track.id);
    });

    row.appendChild(info);
    row.appendChild(playBtn);
    row.appendChild(addBtn);
    row.appendChild(plBtn);
    browseList.appendChild(row);
  }
}

searchInput.addEventListener('input', () => {
  if (searchDebounce) clearTimeout(searchDebounce);
  searchDebounce = setTimeout(() => {
    performSearch(searchInput.value);
  }, 300);
});

// --- Playlist detail ---
async function loadPlaylistDetail(name: string): Promise<void> {
  detailTracks.innerHTML = '';
  detailSubheader.textContent = 'Loading...';

  try {
    let tracks: QueuedTrack[] = [];
    for await (const event of playlist.show(name)) {
      if (event.type === 'queue') {
        tracks = (event as MonoEventQueue).tracks;
      }
    }

    detailSubheader.textContent = `${tracks.length} track${tracks.length !== 1 ? 's' : ''}`;
    detailTracks.innerHTML = '';

    for (let i = 0; i < tracks.length; i++) {
      const track = tracks[i];
      const row = document.createElement('div');
      row.className = 'list-row';
      row.addEventListener('click', () => {
        rpcFire(player.play(track.id));
        navigateTo('now-playing');
      });

      const info = document.createElement('div');
      info.className = 'list-row-info';

      const titleSpan = document.createElement('div');
      titleSpan.className = 'list-row-title';
      titleSpan.textContent = track.title;

      const sub = document.createElement('div');
      sub.className = 'list-row-sub';
      sub.textContent = `${track.artist} · ${formatTime(track.durationSecs)}`;

      // Remove from playlist button
      const removeBtn = document.createElement('button');
      removeBtn.className = 'row-action';
      removeBtn.title = 'Remove from playlist';
      removeBtn.innerHTML = '<svg width="14" height="14" viewBox="0 0 24 24" fill="currentColor"><path d="M19 6.41L17.59 5 12 10.59 6.41 5 5 6.41 10.59 12 5 17.59 6.41 19 12 13.41 17.59 19 19 17.59 13.41 12z"/></svg>';
      const trackIndex = i;
      removeBtn.addEventListener('click', (e) => {
        e.stopPropagation();
        if (currentPlaylistName) {
          rpcFire(playlist.remove(trackIndex, currentPlaylistName));
          row.remove();
          cachedPlaylists = null; // Invalidate cache
        }
      });

      info.appendChild(titleSpan);
      info.appendChild(sub);
      row.appendChild(info);
      row.appendChild(removeBtn);
      detailTracks.appendChild(row);
    }
  } catch {
    detailSubheader.textContent = '';
    detailTracks.innerHTML = '';
    const errEl = document.createElement('div');
    errEl.className = 'list-empty';
    errEl.textContent = 'Failed to load playlist';
    detailTracks.appendChild(errEl);
  }
}

// --- Queue view ---
async function loadQueue(): Promise<void> {
  queueTracksEl.innerHTML = '';
  queueSubheader.textContent = 'Loading...';

  try {
    let tracks: QueuedTrack[] = [];
    let currentIndex: number | null = null;
    for await (const event of player.queueGet()) {
      if (event.type === 'queue') {
        const q = event as MonoEventQueue;
        tracks = q.tracks;
        currentIndex = q.currentIndex;
      }
    }

    if (tracks.length === 0) {
      queueSubheader.textContent = '';
      queueTracksEl.innerHTML = '';
      const emptyEl = document.createElement('div');
      emptyEl.className = 'list-empty';
      emptyEl.textContent = 'Queue is empty';
      queueTracksEl.appendChild(emptyEl);
      return;
    }

    queueSubheader.textContent = `${tracks.length} track${tracks.length !== 1 ? 's' : ''}`;
    queueTracksEl.innerHTML = '';

    for (let i = 0; i < tracks.length; i++) {
      const track = tracks[i];
      const row = document.createElement('div');
      row.className = 'list-row';
      if (i === currentIndex) row.classList.add('active');

      row.addEventListener('click', () => {
        rpcFire(player.play(track.id));
        navigateTo('now-playing');
      });

      const info = document.createElement('div');
      info.className = 'list-row-info';

      const titleSpan = document.createElement('div');
      titleSpan.className = 'list-row-title';
      titleSpan.textContent = track.title;

      const sub = document.createElement('div');
      sub.className = 'list-row-sub';
      sub.textContent = `${track.artist} · ${formatTime(track.durationSecs)}`;

      info.appendChild(titleSpan);
      info.appendChild(sub);
      row.appendChild(info);
      queueTracksEl.appendChild(row);
    }

    // Save queue as playlist button
    const saveBtn = document.createElement('button');
    saveBtn.className = 'save-queue-btn';
    saveBtn.innerHTML = '<svg width="12" height="12" viewBox="0 0 24 24" fill="currentColor"><path d="M17 3H5c-1.11 0-2 .9-2 2v14c0 1.1.89 2 2 2h14c1.1 0 2-.9 2-2V7l-4-4zm-5 16c-1.66 0-3-1.34-3-3s1.34-3 3-3 3 1.34 3 3-1.34 3-3 3zm3-10H5V5h10v4z"/></svg> Save as Playlist';
    saveBtn.addEventListener('click', () => {
      const name = prompt('Playlist name:');
      if (name?.trim()) {
        rpcFire(playlist.save(name.trim()));
        cachedPlaylists = null;
        if (notificationsEnabled) {
          sendNotification({ title: 'Playlist Saved', body: name.trim() });
        }
      }
    });
    queueTracksEl.appendChild(saveBtn);
  } catch {
    queueSubheader.textContent = '';
    queueTracksEl.innerHTML = '';
    const errEl = document.createElement('div');
    errEl.className = 'list-empty';
    errEl.textContent = 'Failed to load queue';
    queueTracksEl.appendChild(errEl);
  }
}

// --- Nav button handlers ---
navBack.addEventListener('click', () => {
  if (currentView === 'browse') {
    navigateTo('now-playing');
  } else if (currentView === 'detail') {
    navigateTo('browse');
  } else if (currentView === 'queue') {
    navigateTo('now-playing');
  } else if (currentView === 'research') {
    navigateTo('browse');
  }
});

navAction.addEventListener('click', () => {
  if (currentView === 'now-playing') {
    navigateTo('browse');
    loadPlaylists();
  } else if (currentView === 'detail' && currentPlaylistName) {
    rpcFire(playlist.play(currentPlaylistName));
    navigateTo('now-playing');
  } else if (currentView === 'queue') {
    rpcFire(player.queueClear());
    queueTracksEl.innerHTML = '';
    queueSubheader.textContent = '';
    const emptyEl = document.createElement('div');
    emptyEl.className = 'list-empty';
    emptyEl.textContent = 'Queue cleared';
    queueTracksEl.appendChild(emptyEl);
  }
});

// Queue button in footer
queueBtn.addEventListener('click', () => {
  navigateTo('queue');
  loadQueue();
});

// --- Transport controls ---
btnPlayPause.addEventListener('click', async () => {
  try {
    const gen = isPlaying ? player.pause() : player.resume();
    for await (const _ of gen) { break; }
  } catch { /* ignore */ }
});

btnPrevious.addEventListener('click', async () => {
  try {
    for await (const _ of player.previous()) { break; }
  } catch { /* ignore */ }
});

btnNext.addEventListener('click', async () => {
  try {
    for await (const _ of player.next()) { break; }
  } catch { /* ignore */ }
});

// Volume control with debounce
volumeSlider.addEventListener('input', () => {
  if (volumeDebounce) clearTimeout(volumeDebounce);
  volumeDebounce = setTimeout(async () => {
    const level = parseInt(volumeSlider.value) / 100;
    try {
      for await (const _ of player.volume(level)) { break; }
    } catch { /* ignore */ }
  }, 50);
});

// Open on Monochrome
openLink.addEventListener('click', async (e) => {
  e.preventDefault();
  const url = openLink.dataset.url;
  if (url && (window as any).__TAURI__) {
    const { open } = await import('@tauri-apps/plugin-shell');
    await open(url);
  } else if (url) {
    window.open(url, '_blank');
  }
});

// --- Playlist CRUD helpers ---
function promptCreatePlaylist(): void {
  const name = prompt('New playlist name:');
  if (name?.trim()) {
    rpcFire(playlist.create(name.trim()));
    cachedPlaylists = null;
    loadPlaylists();
  }
}

function showPlaylistPicker(trackId: number): void {
  pendingAddTrackId = trackId;
  playlistPickerList.innerHTML = '';

  const playlists = cachedPlaylists || [];
  for (const pl of playlists) {
    const row = document.createElement('div');
    row.className = 'list-row';
    const info = document.createElement('div');
    info.className = 'list-row-info';
    const titleSpan = document.createElement('div');
    titleSpan.className = 'list-row-title';
    titleSpan.textContent = pl.name;
    info.appendChild(titleSpan);
    row.appendChild(info);
    row.addEventListener('click', () => {
      rpcFire(playlist.add(trackId, pl.name));
      playlistPicker.classList.add('hidden');
      cachedPlaylists = null;
    });
    playlistPickerList.appendChild(row);
  }

  playlistPicker.classList.remove('hidden');
}

playlistPickerNew.addEventListener('click', () => {
  const name = prompt('New playlist name:');
  if (name?.trim() && pendingAddTrackId !== null) {
    rpcFire(playlist.create(name.trim(), null, [pendingAddTrackId]));
    cachedPlaylists = null;
  }
  playlistPicker.classList.add('hidden');
});

// Close picker on background click
playlistPicker.addEventListener('click', (e) => {
  if (e.target === playlistPicker) {
    playlistPicker.classList.add('hidden');
  }
});

// --- Rename playlist via nav title click ---
navTitle.addEventListener('dblclick', () => {
  if (currentView === 'detail' && currentPlaylistName) {
    const newName = prompt('Rename playlist:', currentPlaylistName);
    if (newName?.trim() && newName.trim() !== currentPlaylistName) {
      rpcFire(playlist.rename(currentPlaylistName, newName.trim()));
      currentPlaylistName = newName.trim();
      navTitle.textContent = currentPlaylistName;
      cachedPlaylists = null;
    }
  }
});

// --- AI Research ---
interface ResearchTrack {
  id: number;
  title: string;
  artist: string;
  reason: string;
}

async function researchPlaylist(query: string): Promise<void> {
  if (isResearching) return;
  isResearching = true;
  sparkleBtn.classList.add('researching');

  try {
    // 1. Search monochrome for tracks matching the query
    const searchResults: MonoEventSearchTrack[] = [];
    for await (const event of mono.search(query, 'tracks', 50)) {
      if (event.type === 'search_track') searchResults.push(event as MonoEventSearchTrack);
    }

    if (searchResults.length === 0) {
      isResearching = false;
      sparkleBtn.classList.remove('researching');
      return;
    }

    // 2. Send to Claude via substrate claudecode for curation
    const trackList = searchResults.map(t => ({ id: t.id, title: t.title, artist: t.artist, album: t.album }));
    const chatPrompt = `Given these tracks from a music search for "${query}", curate a playlist. Select the best tracks that fit together, explain why each fits, and suggest a playlist name. Return ONLY valid JSON, no markdown: {"name": "...", "tracks": [{"id": N, "title": "...", "artist": "...", "reason": "..."}]}

Available tracks: ${JSON.stringify(trackList)}`;

    await substrateRpc.connect();

    // Ensure session exists (create if needed, ignore error if already exists)
    try {
      await claudecode.create('haiku', 'mono-tray-research', '.', false, 'You are a music curator. Return only valid JSON, no markdown fences.');
    } catch { /* session may already exist */ }

    let fullResponse = '';
    for await (const event of claudecode.chat('mono-tray-research', chatPrompt)) {
      if (event.type === 'content') fullResponse += event.text;
    }

    // 3. Parse JSON from response
    const jsonMatch = fullResponse.match(/\{[\s\S]*\}/);
    if (jsonMatch) {
      researchResult = JSON.parse(jsonMatch[0]);
    }
  } catch (err) {
    console.error('Research failed:', err);
  }

  isResearching = false;
  sparkleBtn.classList.remove('researching');

  if (researchResult) {
    sparkleBadge.classList.remove('hidden');
    if (notificationsEnabled) {
      sendNotification({ title: 'Playlist Research', body: `Ready: ${researchResult.name}` });
    }
  }
}

function showResearchResults(): void {
  if (!researchResult) return;
  sparkleBadge.classList.add('hidden');
  researchNameInput.value = researchResult.name;
  researchTracksEl.innerHTML = '';

  for (const track of researchResult.tracks) {
    const row = document.createElement('div');
    row.className = 'list-row';

    const info = document.createElement('div');
    info.className = 'list-row-info';

    const titleSpan = document.createElement('div');
    titleSpan.className = 'list-row-title';
    titleSpan.textContent = track.title;

    const sub = document.createElement('div');
    sub.className = 'list-row-sub';
    sub.textContent = track.artist;

    const reason = document.createElement('div');
    reason.className = 'research-reason';
    reason.textContent = track.reason;

    info.appendChild(titleSpan);
    info.appendChild(sub);
    info.appendChild(reason);

    // Play button
    const playBtn = document.createElement('button');
    playBtn.className = 'row-action';
    playBtn.title = 'Play';
    playBtn.innerHTML = '<svg width="16" height="16" viewBox="0 0 24 24" fill="currentColor"><path d="M8 5v14l11-7z"/></svg>';
    playBtn.addEventListener('click', (e) => {
      e.stopPropagation();
      rpcFire(player.play(track.id));
    });

    // Queue button
    const queueAddBtn = document.createElement('button');
    queueAddBtn.className = 'row-action';
    queueAddBtn.title = 'Add to queue';
    queueAddBtn.innerHTML = '<svg width="16" height="16" viewBox="0 0 24 24" fill="currentColor"><path d="M19 13h-6v6h-2v-6H5v-2h6V5h2v6h6v2z"/></svg>';
    queueAddBtn.addEventListener('click', (e) => {
      e.stopPropagation();
      rpcFire(player.queueAdd(track.id));
    });

    // Remove button
    const removeBtn = document.createElement('button');
    removeBtn.className = 'row-action';
    removeBtn.title = 'Remove';
    removeBtn.innerHTML = '<svg width="14" height="14" viewBox="0 0 24 24" fill="currentColor"><path d="M19 6.41L17.59 5 12 10.59 6.41 5 5 6.41 10.59 12 5 17.59 6.41 19 12 13.41 17.59 19 19 17.59 13.41 12z"/></svg>';
    removeBtn.addEventListener('click', (e) => {
      e.stopPropagation();
      if (researchResult) {
        researchResult.tracks = researchResult.tracks.filter(t => t.id !== track.id);
        row.remove();
      }
    });

    row.appendChild(info);
    row.appendChild(playBtn);
    row.appendChild(queueAddBtn);
    row.appendChild(removeBtn);
    researchTracksEl.appendChild(row);
  }

  navigateTo('research');
}

// Research view buttons
researchCreateBtn.addEventListener('click', () => {
  if (!researchResult) return;
  const name = researchNameInput.value.trim() || researchResult.name;
  const trackIds = researchResult.tracks.map(t => t.id);
  rpcFire(playlist.create(name, null, trackIds));
  cachedPlaylists = null;
  researchResult = null;
  navigateTo('browse');
  loadPlaylists();
});

researchQueueBtn.addEventListener('click', () => {
  if (!researchResult) return;
  for (const track of researchResult.tracks) {
    rpcFire(player.queueAdd(track.id));
  }
});

// Sparkle button: show research results or no-op
sparkleBtn.addEventListener('click', () => {
  if (researchResult) {
    showResearchResults();
  } else if (!isResearching && searchInput.value.trim()) {
    researchPlaylist(searchInput.value.trim());
  }
});

// Double-enter triggers AI research
searchInput.addEventListener('keydown', (e) => {
  if (e.key === 'Enter') {
    const now = Date.now();
    if (now - lastEnterTime < 400) {
      // Double enter
      e.preventDefault();
      if (searchInput.value.trim()) {
        researchPlaylist(searchInput.value.trim());
      }
    }
    lastEnterTime = now;
  }
});

// --- Main loop: stream now_playing with reconnection ---
async function streamNowPlaying(): Promise<void> {
  while (true) {
    try {
      await rpc.connect();
      disconnectOverlay.classList.add('hidden');

      for await (const event of player.nowPlaying()) {
        if (event.type === 'now_playing') {
          updateUI(event as MonoEventNowPlaying);
        }
      }
    } catch (err) {
      console.error('Stream error:', err);
    }

    // Disconnected — show overlay and retry
    disconnectOverlay.classList.remove('hidden');
    rpc.disconnect();
    cachedPlaylists = null;
    await new Promise(r => setTimeout(r, 2000));
  }
}

// --- JS hover polyfill (CSS :hover doesn't fire in NSPanel WebView) ---
// Native global mouseMoved monitor in Rust emits coordinates via Tauri events.
// We use elementFromPoint to resolve the hovered element.
const hoverSelectors = '.nav-btn, .control-btn, .list-row, .row-action, #queue-btn, #open-link, .crumb, #progress-bar, .action-btn, .new-playlist-row, .save-queue-btn';
let currentHover: Element | null = null;

listen<{ x: number; y: number }>('mono-tray://mousemove', (event) => {
  const { x, y } = event.payload;
  const el = document.elementFromPoint(x, y);
  const target = el?.closest?.(hoverSelectors) ?? null;
  if (target !== currentHover) {
    currentHover?.classList.remove('hover');
    target?.classList.add('hover');
    currentHover = target;
  }
});

listen('mono-tray://mouseleave', () => {
  currentHover?.classList.remove('hover');
  currentHover = null;
});

// --- Click feedback: add .clicked class that lingers and fades out ---
// Use 'click' event (not mousedown) since click events fire reliably in NSPanel
const clickSelectors = '.nav-btn, .control-btn, .list-row, .row-action, #queue-btn, .action-btn, .new-playlist-row, .save-queue-btn';
document.addEventListener('click', (e) => {
  const el = e.target as Element;
  const target = el.closest?.(clickSelectors);
  if (target) {
    target.classList.add('clicked');
    setTimeout(() => target.classList.remove('clicked'), 200);
  }
}, true);

// Initialize nav to now-playing
navigateTo('now-playing');

streamNowPlaying();
