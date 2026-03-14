import { PlexusRpcClient } from '../generated/transport';
import { createPlayerClient } from '../generated/player/client';
import { createPlayerPlaylistClient } from '../generated/player/playlist/client';
import { extractData } from '../generated/rpc';
import type { MonoEvent, MonoEventNowPlaying, MonoEventCover, MonoEventPlaylistInfo, MonoEventSearchTrack, MonoEventQueue, QueuedTrack } from '../generated/player/types';
import { isPermissionGranted, requestPermission, sendNotification } from '@tauri-apps/plugin-notification';

// --- Show/hide animations + click-away dismiss ---
import { listen } from '@tauri-apps/api/event';
import { getCurrentWindow } from '@tauri-apps/api/window';

const appEl = document.getElementById('app')!;
const appWindow = getCurrentWindow();
let hiding = false;

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
const queueInfo = document.getElementById('queue-info')!;
const openLink = document.getElementById('open-link') as HTMLAnchorElement;
const disconnectOverlay = document.getElementById('disconnect-overlay')!;

// Nav elements
const navBack = document.getElementById('nav-back')!;
const navTitle = document.getElementById('nav-title')!;
const navAction = document.getElementById('nav-action')!;
const iconList = document.getElementById('icon-list')!;
const iconPlayAll = document.getElementById('icon-play-all')!;
const viewContainer = document.getElementById('view-container')!;
const searchInput = document.getElementById('search-input') as HTMLInputElement;
const browseList = document.getElementById('browse-list')!;
const detailSubheader = document.getElementById('detail-subheader')!;
const detailTracks = document.getElementById('detail-tracks')!;

// --- State ---
let currentTrackId: number | null = null;
let isPlaying = false;
let volumeDebounce: ReturnType<typeof setTimeout> | null = null;
let notificationsEnabled = false;
let currentView: 'now-playing' | 'browse' | 'detail' = 'now-playing';
let cachedPlaylists: MonoEventPlaylistInfo[] | null = null;
let currentPlaylistName: string | null = null;
let searchDebounce: ReturnType<typeof setTimeout> | null = null;
let activeSearchGen: AsyncGenerator | null = null;
let browseScrollTop = 0;

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

// Wrapper for monochrome activation (codegen uses wrong 'mono' prefix)
async function* monoCall(method: string, params: Record<string, unknown> = {}): AsyncGenerator<MonoEvent> {
  yield* extractData<MonoEvent>(rpc.call(`monochrome.${method}`, params));
}

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
    for await (const event of monoCall('cover', { id: trackId, size: 320 })) {
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
function navigateTo(view: 'now-playing' | 'browse' | 'detail'): void {
  // Save browse scroll before leaving
  if (currentView === 'browse' && view !== 'browse') {
    browseScrollTop = browseList.scrollTop;
  }

  currentView = view;
  viewContainer.dataset.view = view;

  // Update nav header
  switch (view) {
    case 'now-playing':
      navBack.classList.add('hidden');
      navTitle.textContent = '';
      iconList.classList.remove('hidden');
      iconPlayAll.classList.add('hidden');
      navAction.classList.remove('hidden');
      navAction.title = 'Library';
      break;
    case 'browse':
      navBack.classList.remove('hidden');
      navTitle.textContent = 'Library';
      iconList.classList.add('hidden');
      iconPlayAll.classList.add('hidden');
      navAction.classList.add('hidden');
      // Restore scroll position
      requestAnimationFrame(() => { browseList.scrollTop = browseScrollTop; });
      break;
    case 'detail':
      navBack.classList.remove('hidden');
      navTitle.textContent = currentPlaylistName || 'Playlist';
      iconList.classList.add('hidden');
      iconPlayAll.classList.remove('hidden');
      navAction.classList.remove('hidden');
      navAction.title = 'Play All';
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
  // Cancel previous search
  if (activeSearchGen) {
    activeSearchGen.return(undefined);
    activeSearchGen = null;
  }

  if (!query.trim()) {
    // Show playlists when search is cleared
    if (cachedPlaylists) {
      renderPlaylistList(cachedPlaylists);
    } else {
      loadPlaylists();
    }
    return;
  }

  const q = query.toLowerCase();

  // Filter playlists locally
  const matchingPlaylists = (cachedPlaylists || []).filter(
    pl => pl.name.toLowerCase().includes(q)
  );

  // Search music provider concurrently
  const gen = monoCall('search', { query, kind: 'tracks', limit: 8 });
  activeSearchGen = gen;

  // Render playlist matches immediately
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
    // Only render if this is still the active search
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

  const chevron = document.createElement('span');
  chevron.className = 'list-row-chevron';
  chevron.innerHTML = '<svg width="14" height="14" viewBox="0 0 24 24" fill="currentColor"><path d="M10 6L8.59 7.41 13.17 12l-4.58 4.59L10 18l6-6z"/></svg>';

  row.appendChild(info);
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

    row.appendChild(info);
    row.appendChild(playBtn);
    row.appendChild(addBtn);
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
      // playlist.show emits playlist_info then a single queue event with all tracks
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

      info.appendChild(titleSpan);
      info.appendChild(sub);
      row.appendChild(info);
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

// --- Nav button handlers ---
navBack.addEventListener('click', () => {
  if (currentView === 'browse') {
    navigateTo('now-playing');
  } else if (currentView === 'detail') {
    navigateTo('browse');
  }
});

navAction.addEventListener('click', () => {
  if (currentView === 'now-playing') {
    navigateTo('browse');
    loadPlaylists();
  } else if (currentView === 'detail' && currentPlaylistName) {
    // Play all
    rpcFire(playlist.play(currentPlaylistName));
    navigateTo('now-playing');
  }
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
    cachedPlaylists = null; // Invalidate cache on disconnect
    await new Promise(r => setTimeout(r, 2000));
  }
}

// Initialize nav to now-playing
navigateTo('now-playing');

streamNowPlaying();
