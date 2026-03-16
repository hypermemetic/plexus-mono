import { PlexusRpcClient } from '../generated/transport';
import { createPlayerClient } from '../generated/player/client';
import { createPlayerPlaylistClient } from '../generated/player/playlist/client';
import { createMonochromeClient } from '../generated/monochrome/client';
import type { MonoEvent, MonoEventNowPlaying, MonoEventCover, MonoEventPlaylistInfo, MonoEventSearchTrack, MonoEventSearchAlbum, MonoEventSearchArtist, MonoEventAlbum, MonoEventAlbumTrack, MonoEventArtist, MonoEventQueue, QueuedTrack } from '../generated/player/types';
import { createClaudecodeClient } from '../generated/substrate/claudecode/client';
import { isPermissionGranted, requestPermission, sendNotification } from '@tauri-apps/plugin-notification';

// --- Show/hide animations + click-away dismiss ---
import { listen } from '@tauri-apps/api/event';
import { getCurrentWindow, LogicalSize } from '@tauri-apps/api/window';

const appEl = document.getElementById('app')!;
const appWindow = getCurrentWindow();
let hiding = false;

type ViewName = 'now-playing' | 'browse' | 'detail' | 'queue' | 'research' | 'history';

const VIEW_HEIGHTS: Record<ViewName, number> = {
  'now-playing': 582,
  'browse': 600,
  'detail': 600,
  'queue': 600,
  'research': 650,
  'history': 600,
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
const detailCover = document.getElementById('detail-cover')!;
const detailCoverImg = document.getElementById('detail-cover-img') as HTMLImageElement;
const detailSubheader = document.getElementById('detail-subheader')!;
const detailTracks = document.getElementById('detail-tracks')!;
const queueSubheader = document.getElementById('queue-subheader')!;
const queueTracksEl = document.getElementById('queue-tracks')!;
const breadcrumbs = document.getElementById('breadcrumbs')!;

// --- State ---
let currentTrackId: number | null = null;
let currentDurationSecs = 0;
let isPlaying = false;
let volumeDebounce: ReturnType<typeof setTimeout> | null = null;
let notificationsEnabled = false;
let currentView: ViewName = 'now-playing';
let cachedPlaylists: MonoEventPlaylistInfo[] | null = null;
let currentPlaylistName: string | null = null;
let currentDetailAlbumId: number | null = null;
let searchDebounce: ReturnType<typeof setTimeout> | null = null;
let activeSearchGen: AsyncGenerator | null = null;
let browseScrollTop = 0;
let lastQueueLength = 0;
let searchKind: 'tracks' | 'albums' | 'artists' = 'tracks';
let lastEnterTime = 0;
let researchResult: { name: string; tracks: ResearchTrack[] } | null = null;
let isResearching = false;
let pendingAddTrackId: number | null = null;
let claudeSessionReady = false;
const likedSet = new Set<number>();

// New DOM elements
const sparkleBtn = document.getElementById('sparkle-btn')!;
const sparkleBadge = document.getElementById('sparkle-badge')!;
const researchStatus = document.getElementById('research-status')!;
const researchNameInput = document.getElementById('research-name') as HTMLInputElement;
const researchTracksEl = document.getElementById('research-tracks')!;
const researchCreateBtn = document.getElementById('research-create-btn')!;
const researchQueueBtn = document.getElementById('research-queue-btn')!;
const playlistPicker = document.getElementById('playlist-picker')!;
const playlistPickerList = document.getElementById('playlist-picker-list')!;
const playlistPickerNew = document.getElementById('playlist-picker-new')!;
const likeBtn = document.getElementById('like-btn')!;
const npDownloadBtn = document.getElementById('np-download-btn')!;
const historyBtn = document.getElementById('history-btn')!;
const historySubheader = document.getElementById('history-subheader')!;
const historyTracksEl = document.getElementById('history-tracks')!;

// --- Waveform state ---
// Rolling waveform: peaks stream in at ~30fps from the audio_peaks endpoint.
// Drawn as a smooth freeform shape floating above the progress bar.
const waveformCanvas = document.getElementById('waveform-canvas') as HTMLCanvasElement;
const waveCtx = waveformCanvas.getContext('2d')!;
const PEAK_BUFFER_SIZE = 200;
const peakBuffer = new Float32Array(PEAK_BUFFER_SIZE);
let peakWriteIndex = 0;
let peakBufferFilled = 0;
let silenceSince: number | null = null;
let waveformAnimId = 0;
let lastWaveformTrackId: number | null = null;

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
  backend: 'music',
  url: 'ws://127.0.0.1:4448',
  debug: false,
});

const player = createPlayerClient(rpc);
const playlist = createPlayerPlaylistClient(rpc);
const mono = createMonochromeClient(rpc);

// Substrate RPC client for AI research (claudecode)
const substrateRpc = new PlexusRpcClient({
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

// --- Cover art cache ---
const coverCache = new Map<number, string>();
const coverInflight = new Map<number, Promise<string | null>>();

async function getCoverUrl(trackId: number): Promise<string | null> {
  const cached = coverCache.get(trackId);
  if (cached) return cached;

  const inflight = coverInflight.get(trackId);
  if (inflight) return inflight;

  const promise = (async (): Promise<string | null> => {
    try {
      for await (const event of mono.cover(trackId, 640)) {
        if (event.type === 'cover') {
          const url = (event as MonoEventCover).url;
          coverCache.set(trackId, url);
          return url;
        }
      }
    } catch { /* no cover */ }
    return null;
  })();

  coverInflight.set(trackId, promise);
  const result = await promise;
  coverInflight.delete(trackId);
  return result;
}

async function fetchCoverArt(trackId: number): Promise<void> {
  const url = await getCoverUrl(trackId);
  if (url) {
    albumArt.src = url;
    albumArt.classList.add('loaded');
  } else {
    albumArt.classList.remove('loaded');
  }
}

const DOWNLOAD_DIR = '~/Music/mono-tray';
const downloadIcon = '<svg width="14" height="14" viewBox="0 0 24 24" fill="currentColor"><path d="M19 9h-4V3H9v6H5l7 7 7-7zM5 18v2h14v-2H5z"/></svg>';
const checkIcon = '<svg width="14" height="14" viewBox="0 0 24 24" fill="currentColor"><path d="M9 16.17L4.83 12l-1.42 1.41L9 19 21 7l-1.41-1.41z"/></svg>';

function makeDownloadBtn(trackId: number): HTMLButtonElement {
  const btn = document.createElement('button');
  btn.className = 'row-action';
  btn.title = 'Download';
  btn.innerHTML = downloadIcon;
  btn.addEventListener('click', async (e) => {
    e.stopPropagation();
    btn.classList.add('downloading');
    btn.innerHTML = '<svg width="14" height="14" viewBox="0 0 24 24" fill="currentColor" class="spin"><circle cx="12" cy="12" r="10" fill="none" stroke="currentColor" stroke-width="2" stroke-dasharray="31.4 31.4" stroke-linecap="round"/></svg>';
    try {
      for await (const event of mono.download(trackId, DOWNLOAD_DIR)) {
        if (event.type === 'download_progress') {
          const pct = (event as any).percent;
          if (pct != null) btn.title = `${Math.round(pct)}%`;
        } else if (event.type === 'download_complete') {
          btn.innerHTML = checkIcon;
          btn.classList.remove('downloading');
          btn.classList.add('done');
          btn.title = 'Downloaded';
          setTimeout(() => {
            btn.innerHTML = downloadIcon;
            btn.classList.remove('done');
            btn.title = 'Download';
          }, 3000);
          return;
        }
      }
    } catch {
      btn.innerHTML = downloadIcon;
      btn.classList.remove('downloading');
      btn.title = 'Download failed';
    }
  });
  return btn;
}

const placeholderSvg = '<svg width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5"><path d="M9 18V5l12-2v13"/><circle cx="6" cy="18" r="3"/><circle cx="18" cy="16" r="3"/></svg>';

/** Create a lazy-loading cover art thumbnail (uses track ID) */
function makeCoverThumb(trackId: number): HTMLDivElement {
  const wrap = document.createElement('div');
  wrap.className = 'cover-thumb loading';
  const img = document.createElement('img');
  img.alt = '';
  const placeholder = document.createElement('div');
  placeholder.className = 'cover-thumb-placeholder';
  placeholder.innerHTML = placeholderSvg;
  wrap.appendChild(placeholder);
  wrap.appendChild(img);
  // Lazy load cover
  getCoverUrl(trackId).then(url => {
    if (url) {
      img.src = url;
      img.onload = () => {
        img.classList.add('loaded');
        wrap.classList.remove('loading');
        wrap.classList.add('has-cover');
      };
    } else {
      wrap.classList.remove('loading');
      wrap.classList.add('failed');
    }
  });
  return wrap;
}

/** Create a cover thumb for an album (loads album to get first track ID) */
function makeAlbumCoverThumb(albumId: number): HTMLDivElement {
  const wrap = document.createElement('div');
  wrap.className = 'cover-thumb loading';
  const img = document.createElement('img');
  img.alt = '';
  const placeholder = document.createElement('div');
  placeholder.className = 'cover-thumb-placeholder';
  placeholder.innerHTML = placeholderSvg;
  wrap.appendChild(placeholder);
  wrap.appendChild(img);
  // Get first track from album, then load its cover
  (async () => {
    try {
      let firstTrackId: number | null = null;
      for await (const event of mono.album(albumId)) {
        if (event.type === 'album_track' && !firstTrackId) {
          firstTrackId = (event as MonoEventAlbumTrack).id;
          break;
        }
      }
      if (firstTrackId) {
        const url = await getCoverUrl(firstTrackId);
        if (url) {
          img.src = url;
          img.onload = () => {
            img.classList.add('loaded');
            wrap.classList.remove('loading');
            wrap.classList.add('has-cover');
          };
          return;
        }
      }
    } catch { /* no cover */ }
    wrap.classList.remove('loading');
    wrap.classList.add('failed');
  })();
  return wrap;
}

// --- Clickable artist/album helpers ---
function makeArtistLink(name: string): HTMLSpanElement {
  const span = document.createElement('span');
  span.className = 'clickable-meta';
  span.textContent = name;
  span.addEventListener('click', (e) => {
    e.stopPropagation();
    currentPlaylistName = name;
    navigateTo('detail');
    loadArtistAlbums(0, name);
  });
  return span;
}

function makeAlbumLink(albumName: string, trackId?: number): HTMLSpanElement {
  const span = document.createElement('span');
  span.className = 'clickable-meta';
  span.textContent = albumName;
  span.addEventListener('click', (e) => {
    e.stopPropagation();
    navigateToAlbum(albumName, trackId);
  });
  return span;
}

async function navigateToAlbum(albumName: string, trackId?: number): Promise<void> {
  currentPlaylistName = albumName;
  navigateTo('detail');
  detailTracks.innerHTML = '';
  detailSubheader.textContent = 'Loading...';

  // If we have a trackId, look up the track to get the real albumId
  if (trackId) {
    try {
      for await (const event of mono.track(trackId)) {
        if (event.type === 'track') {
          loadAlbumDetail((event as import('../generated/player/types').MonoEventTrack).albumId);
          return;
        }
      }
    } catch { /* fall through to search */ }
  }

  // Fallback: search by album name
  for await (const event of mono.search(albumName, 'albums', 5)) {
    if (event.type === 'search_album') {
      const album = event as MonoEventSearchAlbum;
      if (album.title.toLowerCase() === albumName.toLowerCase()) {
        loadAlbumDetail(album.id);
        return;
      }
    }
  }
  // Last resort: first result
  for await (const event of mono.search(albumName, 'albums', 1)) {
    if (event.type === 'search_album') {
      loadAlbumDetail((event as MonoEventSearchAlbum).id);
      return;
    }
  }
  detailSubheader.textContent = 'Album not found';
}

/** Build a subtitle element with clickable artist/album spans */
function makeTrackSub(artist: string, opts?: { album?: string; durationSecs?: number; trackId?: number }): HTMLDivElement {
  const sub = document.createElement('div');
  sub.className = 'list-row-sub';

  sub.appendChild(makeArtistLink(artist));

  if (opts?.album) {
    const sep = document.createTextNode(' — ');
    sub.appendChild(sep);
    sub.appendChild(makeAlbumLink(opts.album, opts.trackId));
  }

  if (opts?.durationSecs != null) {
    const sep = document.createTextNode(' · ');
    sub.appendChild(sep);
    const dur = document.createTextNode(formatTime(opts.durationSecs));
    sub.appendChild(dur);
  }

  return sub;
}

function updatePlayPauseIcon(status: string): void {
  isPlaying = status === 'playing' || status === 'buffering' || status === 'starting';
  iconPlay.classList.toggle('hidden', isPlaying);
  iconPause.classList.toggle('hidden', !isPlaying);
}

function updateUI(np: MonoEventNowPlaying): void {
  titleEl.textContent = np.title || 'Not Playing';

  // Clickable artist/album in now-playing
  artistAlbumEl.innerHTML = '';
  if (np.artist) {
    artistAlbumEl.appendChild(makeArtistLink(np.artist));
    if (np.album) {
      artistAlbumEl.appendChild(document.createTextNode(' — '));
      artistAlbumEl.appendChild(makeAlbumLink(np.album, np.trackId));
    }
  } else if (np.album) {
    artistAlbumEl.appendChild(makeAlbumLink(np.album, np.trackId));
  }

  currentDurationSecs = np.durationSecs;
  if (!scrubbing) {
    const pct = np.durationSecs > 0 ? (np.positionSecs / np.durationSecs) * 100 : 0;
    progressFill.style.width = `${pct}%`;
    progressThumb.style.left = `${pct}%`;
    timeCurrent.textContent = formatTime(np.positionSecs);
  }
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

  // Like button state
  if (np.trackId) {
    likeBtn.classList.toggle('hidden', false);
    npDownloadBtn.classList.toggle('hidden', false);
    const liked = (np as any).isLiked === true;
    likeBtn.classList.toggle('liked', liked);
    if (liked) likedSet.add(np.trackId);
    else likedSet.delete(np.trackId);

    const downloaded = (np as any).isDownloaded === true;
    npDownloadBtn.classList.toggle('downloaded', downloaded);
  } else {
    likeBtn.classList.add('hidden');
    npDownloadBtn.classList.add('hidden');
  }

  if (np.trackId && np.trackId !== currentTrackId) {
    currentTrackId = np.trackId;
    fetchCoverArt(np.trackId);
    notifyTrackChange(np);
  } else if (!np.trackId) {
    currentTrackId = null;
    albumArt.classList.remove('loaded');
  }

  // --- Waveform: reset buffer on track change ---
  if (np.trackId !== lastWaveformTrackId) {
    lastWaveformTrackId = np.trackId ?? null;
    peakBuffer.fill(0);
    peakWriteIndex = 0;
    peakBufferFilled = 0;
    silenceSince = null;
  }
}

// --- Waveform drawing ---
// Smooth freeform shape from rolling peak buffer, floating above progress bar.
function drawWaveform(): void {
  waveformAnimId = requestAnimationFrame(drawWaveform);

  const w = waveformCanvas.width;
  const h = waveformCanvas.height;
  waveCtx.clearRect(0, 0, w, h);

  if (!isPlaying || peakBufferFilled < 2) return;

  // Dotted line while scrubbing
  if (scrubbing) {
    waveCtx.setLineDash([4, 4]);
    waveCtx.strokeStyle = '#888';
    waveCtx.lineWidth = 1;
    waveCtx.beginPath();
    waveCtx.moveTo(0, h - 1);
    waveCtx.lineTo(w, h - 1);
    waveCtx.stroke();
    waveCtx.setLineDash([]);
    return;
  }

  const silenceWarning = silenceSince !== null && (Date.now() - silenceSince) > 5000;
  const count = Math.min(peakBufferFilled, PEAK_BUFFER_SIZE);
  const step = w / (PEAK_BUFFER_SIZE - 1);
  const accentColor = silenceWarning ? 'rgba(231, 76, 60, 0.7)' : 'rgba(29, 185, 84, 0.6)';

  // Build points array: oldest → newest, left → right
  const points: { x: number; y: number }[] = [];
  for (let i = 0; i < count; i++) {
    const bufIdx = (peakWriteIndex - count + i + PEAK_BUFFER_SIZE) % PEAK_BUFFER_SIZE;
    const peak = peakBuffer[bufIdx];
    const x = (PEAK_BUFFER_SIZE - count + i) * step;
    const y = h - Math.max(1, peak * h * 0.85);
    points.push({ x, y });
  }

  // Draw filled smooth curve using quadratic bezier through midpoints
  waveCtx.beginPath();
  waveCtx.moveTo(points[0].x, h); // start at bottom-left
  waveCtx.lineTo(points[0].x, points[0].y);

  for (let i = 0; i < points.length - 1; i++) {
    const mx = (points[i].x + points[i + 1].x) / 2;
    const my = (points[i].y + points[i + 1].y) / 2;
    waveCtx.quadraticCurveTo(points[i].x, points[i].y, mx, my);
  }

  // Final point
  const last = points[points.length - 1];
  waveCtx.lineTo(last.x, last.y);
  waveCtx.lineTo(last.x, h); // down to bottom-right
  waveCtx.closePath();

  // Gradient fill: accent at top, fading to transparent at bottom
  const grad = waveCtx.createLinearGradient(0, 0, 0, h);
  grad.addColorStop(0, accentColor);
  grad.addColorStop(1, 'rgba(29, 185, 84, 0.05)');
  waveCtx.fillStyle = silenceWarning ? 'rgba(231, 76, 60, 0.3)' : grad;
  waveCtx.fill();

  // Stroke the top edge for definition
  waveCtx.beginPath();
  waveCtx.moveTo(points[0].x, points[0].y);
  for (let i = 0; i < points.length - 1; i++) {
    const mx = (points[i].x + points[i + 1].x) / 2;
    const my = (points[i].y + points[i + 1].y) / 2;
    waveCtx.quadraticCurveTo(points[i].x, points[i].y, mx, my);
  }
  waveCtx.lineTo(last.x, last.y);
  waveCtx.strokeStyle = silenceWarning ? '#e74c3c' : '#1db954';
  waveCtx.lineWidth = 1.5;
  waveCtx.stroke();

  if (silenceWarning) {
    waveCtx.fillStyle = '#e74c3c';
    waveCtx.font = '10px -apple-system, sans-serif';
    waveCtx.textAlign = 'center';
    waveCtx.fillText('No audio', w / 2, h / 2 + 3);
  }
}

// Start waveform animation loop
drawWaveform();

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
  } else if (view === 'history') {
    trail.push({ label: 'Now Playing', view: 'now-playing' });
    trail.push({ label: 'History', current: true });
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
    case 'history':
      navBack.classList.remove('hidden');
      navTitle.textContent = 'History';
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

  // Render Liked playlist at top if present (real backend file)
  const likedIdx = playlists.findIndex(pl => pl.name === 'Liked');
  if (likedIdx >= 0) {
    const likedPl = playlists[likedIdx];
    const likedRow = document.createElement('div');
    likedRow.className = 'liked-songs-row';
    likedRow.innerHTML = `
      <div class="liked-songs-icon">
        <svg width="20" height="20" viewBox="0 0 24 24" fill="white"><path d="M12 21.35l-1.45-1.32C5.4 15.36 2 12.28 2 8.5 2 5.42 4.42 3 7.5 3c1.74 0 3.41.81 4.5 2.09C13.09 3.81 14.76 3 16.5 3 19.58 3 22 5.42 22 8.5c0 3.78-3.4 6.86-8.55 11.54L12 21.35z"/></svg>
      </div>
      <div class="list-row-info">
        <div class="list-row-title">Liked Songs</div>
        <div class="list-row-sub">${likedPl.trackCount} track${likedPl.trackCount !== 1 ? 's' : ''}</div>
      </div>
    `;
    likedRow.addEventListener('click', () => {
      currentPlaylistName = 'Liked';
      navigateTo('detail');
      loadPlaylistDetail('Liked');
    });
    browseList.appendChild(likedRow);
  }

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
    if (pl.name === 'Liked') continue; // rendered above with special styling
    browseList.appendChild(makePlaylistRow(pl));
  }
}

// --- Search ---
const searchTabs = document.getElementById('search-tabs')!;

function setSearchKind(kind: 'tracks' | 'albums' | 'artists'): void {
  searchKind = kind;
  searchTabs.querySelectorAll('.search-tab').forEach(tab => {
    tab.classList.toggle('active', (tab as HTMLElement).dataset.kind === kind);
  });
  searchInput.placeholder = `Search ${kind}...`;
  if (searchInput.value.trim()) performSearch(searchInput.value);
}

searchTabs.addEventListener('click', (e) => {
  const tab = (e.target as HTMLElement).closest('.search-tab') as HTMLElement | null;
  if (tab?.dataset.kind) setSearchKind(tab.dataset.kind as any);
});

function performSearch(query: string): void {
  if (activeSearchGen) {
    activeSearchGen.return(undefined);
    activeSearchGen = null;
  }

  if (!query.trim()) {
    searchTabs.classList.add('hidden');
    if (cachedPlaylists) {
      renderPlaylistList(cachedPlaylists);
    } else {
      loadPlaylists();
    }
    return;
  }

  searchTabs.classList.remove('hidden');

  const q = query.toLowerCase();
  const matchingPlaylists = (cachedPlaylists || []).filter(
    pl => pl.name.toLowerCase().includes(q)
  );

  const gen = mono.search(query, searchKind, 12);
  activeSearchGen = gen;

  browseList.innerHTML = '';
  if (matchingPlaylists.length > 0 && searchKind === 'tracks') {
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
    const results: MonoEvent[] = [];
    try {
      for await (const event of gen) {
        if (event.type === 'search_track' || event.type === 'search_album' || event.type === 'search_artist') {
          results.push(event);
        }
      }
    } catch {
      // Search cancelled or failed
    }
    if (activeSearchGen === gen) {
      if (searchKind === 'tracks') {
        renderSearchResults(results as MonoEventSearchTrack[], matchingPlaylists.length > 0);
      } else if (searchKind === 'albums') {
        renderAlbumResults(results as MonoEventSearchAlbum[]);
      } else {
        renderArtistResults(results as MonoEventSearchArtist[]);
      }
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
  const trackLabel = `${pl.trackCount} track${pl.trackCount !== 1 ? 's' : ''}`;
  if (pl.description) {
    sub.innerHTML = `<span class="ai-badge">AI</span> ${trackLabel}`;
  } else {
    sub.textContent = trackLabel;
  }

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

    const sub = makeTrackSub(track.artist, { album: track.album, durationSecs: track.durationSecs, trackId: track.id });

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
    row.appendChild(makeDownloadBtn(track.id));
    browseList.appendChild(row);
  }
}

function renderAlbumResults(results: MonoEventSearchAlbum[]): void {
  browseList.innerHTML = '';
  if (results.length === 0) {
    const emptyEl = document.createElement('div');
    emptyEl.className = 'list-empty';
    emptyEl.textContent = 'No albums found';
    browseList.appendChild(emptyEl);
    return;
  }
  for (const album of results) {
    const row = document.createElement('div');
    row.className = 'list-row';
    row.addEventListener('click', () => {
      currentPlaylistName = album.title;
      navigateTo('detail');
      loadAlbumDetail(album.id);
    });

    const info = document.createElement('div');
    info.className = 'list-row-info';

    const titleSpan = document.createElement('div');
    titleSpan.className = 'list-row-title';
    titleSpan.textContent = album.title;

    const sub = document.createElement('div');
    sub.className = 'list-row-sub';
    const parts = [album.artist, `${album.trackCount} tracks`];
    if (album.releaseDate) parts.push(album.releaseDate.slice(0, 4));
    sub.textContent = parts.join(' · ');

    info.appendChild(titleSpan);
    info.appendChild(sub);

    const chevron = document.createElement('span');
    chevron.className = 'list-row-chevron';
    chevron.innerHTML = '<svg width="14" height="14" viewBox="0 0 24 24" fill="currentColor"><path d="M10 6L8.59 7.41 13.17 12l-4.58 4.59L10 18l6-6z"/></svg>';

    row.appendChild(makeAlbumCoverThumb(album.id));
    row.appendChild(info);
    row.appendChild(chevron);
    browseList.appendChild(row);
  }
}

function renderArtistResults(results: MonoEventSearchArtist[]): void {
  browseList.innerHTML = '';
  if (results.length === 0) {
    const emptyEl = document.createElement('div');
    emptyEl.className = 'list-empty';
    emptyEl.textContent = 'No artists found';
    browseList.appendChild(emptyEl);
    return;
  }
  for (const artist of results) {
    const row = document.createElement('div');
    row.className = 'list-row';
    row.addEventListener('click', () => {
      currentPlaylistName = artist.name;
      navigateTo('detail');
      loadArtistAlbums(artist.id, artist.name);
    });

    const info = document.createElement('div');
    info.className = 'list-row-info';

    const titleSpan = document.createElement('div');
    titleSpan.className = 'list-row-title';
    titleSpan.textContent = artist.name;

    info.appendChild(titleSpan);

    const chevron = document.createElement('span');
    chevron.className = 'list-row-chevron';
    chevron.innerHTML = '<svg width="14" height="14" viewBox="0 0 24 24" fill="currentColor"><path d="M10 6L8.59 7.41 13.17 12l-4.58 4.59L10 18l6-6z"/></svg>';

    row.appendChild(info);
    row.appendChild(chevron);
    browseList.appendChild(row);
  }
}

// --- Album detail (reuses detail view) ---
async function loadAlbumDetail(albumId: number): Promise<void> {
  currentDetailAlbumId = albumId;
  detailTracks.innerHTML = '';
  detailSubheader.textContent = 'Loading...';
  detailCover.classList.remove('loaded');
  detailCover.classList.add('loading');
  detailCover.style.display = 'flex';

  try {
    let albumInfo: MonoEventAlbum | null = null;
    const tracks: MonoEventAlbumTrack[] = [];
    for await (const event of mono.album(albumId)) {
      if (event.type === 'album') albumInfo = event as MonoEventAlbum;
      if (event.type === 'album_track') tracks.push(event as MonoEventAlbumTrack);
    }

    // Load cover art from first track (mono.cover needs track IDs, not album IDs)
    if (tracks.length > 0) {
      getCoverUrl(tracks[0].id).then(url => {
        if (url) {
          detailCoverImg.src = url;
          detailCoverImg.onload = () => {
            detailCover.classList.remove('loading');
            detailCover.classList.add('loaded');
          };
        } else {
          detailCover.classList.remove('loading');
          detailCover.style.display = 'none';
        }
      });
    } else {
      detailCover.classList.remove('loading');
      detailCover.style.display = 'none';
    }

    if (albumInfo) {
      currentPlaylistName = albumInfo.title;
      navTitle.textContent = albumInfo.title;
      const parts = [albumInfo.artist, `${tracks.length} tracks`];
      if (albumInfo.releaseDate) parts.push(albumInfo.releaseDate.slice(0, 4));
      detailSubheader.textContent = parts.join(' · ');
    } else {
      detailSubheader.textContent = `${tracks.length} tracks`;
    }

    detailTracks.innerHTML = '';

    // Download all button
    if (tracks.length > 0) {
      const dlAllBtn = document.createElement('button');
      dlAllBtn.className = 'save-queue-btn';
      dlAllBtn.innerHTML = downloadIcon + ' Download Album';
      dlAllBtn.addEventListener('click', async () => {
        dlAllBtn.textContent = 'Downloading...';
        dlAllBtn.classList.add('downloading');
        for (const t of tracks) {
          try {
            for await (const ev of mono.download(t.id, DOWNLOAD_DIR)) {
              if (ev.type === 'download_complete') break;
            }
          } catch { /* continue with next track */ }
        }
        dlAllBtn.innerHTML = checkIcon + ' Downloaded';
        dlAllBtn.classList.remove('downloading');
        setTimeout(() => { dlAllBtn.innerHTML = downloadIcon + ' Download Album'; }, 3000);
      });
      detailTracks.appendChild(dlAllBtn);
    }

    for (const track of tracks) {
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
      titleSpan.textContent = `${track.position}. ${track.title}`;

      const sub = makeTrackSub(track.artist, { durationSecs: track.durationSecs });

      info.appendChild(titleSpan);
      info.appendChild(sub);

      // Queue button
      const queueBtn = document.createElement('button');
      queueBtn.className = 'row-action';
      queueBtn.title = 'Add to queue';
      queueBtn.innerHTML = '<svg width="16" height="16" viewBox="0 0 24 24" fill="currentColor"><path d="M19 13h-6v6h-2v-6H5v-2h6V5h2v6h6v2z"/></svg>';
      queueBtn.addEventListener('click', (e) => {
        e.stopPropagation();
        rpcFire(player.queueAdd(track.id));
      });

      row.appendChild(info);
      row.appendChild(queueBtn);
      row.appendChild(makeDownloadBtn(track.id));
      detailTracks.appendChild(row);
    }
  } catch {
    detailSubheader.textContent = '';
    detailTracks.innerHTML = '';
    const errEl = document.createElement('div');
    errEl.className = 'list-empty';
    errEl.textContent = 'Failed to load album';
    detailTracks.appendChild(errEl);
  }
}

// --- Artist albums (reuses detail view) ---
async function loadArtistAlbums(artistId: number, artistName: string): Promise<void> {
  currentDetailAlbumId = null;
  detailTracks.innerHTML = '';
  detailSubheader.textContent = 'Loading...';
  detailCover.classList.remove('loaded');
  navTitle.textContent = artistName;

  try {
    // Search for albums by this artist
    const albums: MonoEventSearchAlbum[] = [];
    for await (const event of mono.search(artistName, 'albums', 20)) {
      if (event.type === 'search_album') {
        const album = event as MonoEventSearchAlbum;
        // Filter to only this artist's albums
        if (album.artist.toLowerCase() === artistName.toLowerCase()) {
          albums.push(album);
        }
      }
    }

    detailSubheader.textContent = `${albums.length} album${albums.length !== 1 ? 's' : ''}`;
    detailTracks.innerHTML = '';

    if (albums.length === 0) {
      const emptyEl = document.createElement('div');
      emptyEl.className = 'list-empty';
      emptyEl.textContent = 'No albums found';
      detailTracks.appendChild(emptyEl);
      return;
    }

    for (const album of albums) {
      const row = document.createElement('div');
      row.className = 'list-row';
      row.addEventListener('click', () => {
        currentPlaylistName = album.title;
        loadAlbumDetail(album.id);
      });

      const info = document.createElement('div');
      info.className = 'list-row-info';

      const titleSpan = document.createElement('div');
      titleSpan.className = 'list-row-title';
      titleSpan.textContent = album.title;

      const sub = document.createElement('div');
      sub.className = 'list-row-sub';
      const parts = [`${album.trackCount} tracks`];
      if (album.releaseDate) parts.push(album.releaseDate.slice(0, 4));
      sub.textContent = parts.join(' · ');

      info.appendChild(titleSpan);
      info.appendChild(sub);

      const chevron = document.createElement('span');
      chevron.className = 'list-row-chevron';
      chevron.innerHTML = '<svg width="14" height="14" viewBox="0 0 24 24" fill="currentColor"><path d="M10 6L8.59 7.41 13.17 12l-4.58 4.59L10 18l6-6z"/></svg>';

      row.appendChild(makeAlbumCoverThumb(album.id));
      row.appendChild(info);
      row.appendChild(chevron);
      detailTracks.appendChild(row);
    }
  } catch {
    detailSubheader.textContent = '';
    detailTracks.innerHTML = '';
    const errEl = document.createElement('div');
    errEl.className = 'list-empty';
    errEl.textContent = 'Failed to load artist';
    detailTracks.appendChild(errEl);
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
  currentDetailAlbumId = null;
  detailTracks.innerHTML = '';
  detailSubheader.textContent = 'Loading...';
  detailCover.classList.remove('loaded');

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

      const sub = makeTrackSub(track.artist, { album: track.album, durationSecs: track.durationSecs, trackId: track.id });

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
      row.appendChild(makeDownloadBtn(track.id));
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

// --- Liked Songs detail ---
async function loadLikedSongsDetail(): Promise<void> {
  currentDetailAlbumId = null;
  detailTracks.innerHTML = '';
  detailSubheader.textContent = 'Loading...';
  detailCover.classList.remove('loaded');
  detailCover.style.display = 'none';

  try {
    let ids: number[] = [];
    for await (const event of player.likedTracks()) {
      if (event.type === 'queue') {
        ids = (event as MonoEventQueue).tracks.map(t => t.id);
      }
    }
    if (ids.length === 0) {
      detailSubheader.textContent = 'No liked songs';
      return;
    }
    detailSubheader.textContent = `${ids.length} track${ids.length !== 1 ? 's' : ''}`;

    // Resolve track metadata via search (best effort)
    for (const id of ids) {
      try {
        let trackEvent: any = null;
        for await (const event of mono.track(id)) {
          if (event.type === 'track') { trackEvent = event; break; }
        }
        if (!trackEvent) continue;
        const row = document.createElement('div');
        row.className = 'list-row';
        row.addEventListener('click', () => {
          rpcFire(player.play(id));
          navigateTo('now-playing');
        });
        const info = document.createElement('div');
        info.className = 'list-row-info';
        const titleSpan = document.createElement('div');
        titleSpan.className = 'list-row-title';
        titleSpan.textContent = trackEvent.title;
        const sub = makeTrackSub(trackEvent.artist, { album: trackEvent.album, durationSecs: trackEvent.durationSecs, trackId: id });
        info.appendChild(titleSpan);
        info.appendChild(sub);

        // Unlike button
        const unlikeBtn = document.createElement('button');
        unlikeBtn.className = 'row-action';
        unlikeBtn.title = 'Unlike';
        unlikeBtn.innerHTML = '<svg width="14" height="14" viewBox="0 0 24 24" fill="#e74c3c"><path d="M12 21.35l-1.45-1.32C5.4 15.36 2 12.28 2 8.5 2 5.42 4.42 3 7.5 3c1.74 0 3.41.81 4.5 2.09C13.09 3.81 14.76 3 16.5 3 19.58 3 22 5.42 22 8.5c0 3.78-3.4 6.86-8.55 11.54L12 21.35z"/></svg>';
        unlikeBtn.addEventListener('click', (e) => {
          e.stopPropagation();
          rpcFire(player.like(id, 'liked-songs'));
          likedSet.delete(id);
          row.remove();
        });

        row.appendChild(info);
        row.appendChild(makeDownloadBtn(id));
        row.appendChild(unlikeBtn);
        detailTracks.appendChild(row);
      } catch { /* skip this track */ }
    }
  } catch {
    detailSubheader.textContent = '';
    detailTracks.innerHTML = '';
    const errEl = document.createElement('div');
    errEl.className = 'list-empty';
    errEl.textContent = 'Failed to load liked songs';
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

      const sub = makeTrackSub(track.artist, { album: track.album, durationSecs: track.durationSecs, trackId: track.id });
      if (track.source) {
        const fromSpan = document.createElement('span');
        fromSpan.className = 'queue-source';
        fromSpan.textContent = ` · ${track.source}`;
        sub.appendChild(fromSpan);
      }

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
  } else if (currentView === 'history') {
    navigateTo('now-playing');
  }
});

navAction.addEventListener('click', () => {
  if (currentView === 'now-playing') {
    navigateTo('browse');
    loadPlaylists();
  } else if (currentView === 'detail' && currentDetailAlbumId) {
    rpcFire(player.queueAlbum(currentDetailAlbumId));
    navigateTo('now-playing');
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

// Progress bar scrubbing
const progressBar = document.getElementById('progress-bar')!;
let scrubbing = false;

function scrubTo(e: MouseEvent | PointerEvent): void {
  const rect = progressBar.getBoundingClientRect();
  const pct = Math.max(0, Math.min(1, (e.clientX - rect.left) / rect.width));
  progressFill.style.width = `${pct * 100}%`;
  progressThumb.style.left = `${pct * 100}%`;
  timeCurrent.textContent = formatTime(pct * currentDurationSecs);
}

progressBar.addEventListener('pointerdown', (e) => {
  if (currentDurationSecs <= 0) return;
  scrubbing = true;
  progressFill.style.transition = 'none';
  progressBar.setPointerCapture(e.pointerId);
  scrubTo(e);
});

progressBar.addEventListener('pointermove', (e) => {
  if (!scrubbing) return;
  scrubTo(e);
});

progressBar.addEventListener('pointerup', (e) => {
  if (!scrubbing) return;
  scrubbing = false;
  progressFill.style.transition = '';
  const rect = progressBar.getBoundingClientRect();
  const pct = Math.max(0, Math.min(1, (e.clientX - rect.left) / rect.width));
  const seekPos = pct * currentDurationSecs;
  rpcFire(player.seek(seekPos));
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

const RESEARCH_SESSION = 'mono-tray-research';
const MAX_RESEARCH_ATTEMPTS = 3;

function setResearchStatus(text: string, isError = false): void {
  researchStatus.textContent = text;
  researchStatus.classList.toggle('error', isError);
  researchStatus.classList.toggle('hidden', !text);
}

async function ensureClaudeSession(): Promise<void> {
  if (claudeSessionReady) return;
  await substrateRpc.connect();
  try {
    await claudecode.create('sonnet', RESEARCH_SESSION, '/tmp', false,
      `You are a music researcher and playlist curator. You have access to WebSearch to research music themes, genres, artists, and tracks.

When asked to research a theme, use WebSearch to find artists, albums, and tracks that match. Then return a JSON object with search terms for finding those in a music catalog.

When asked to curate from found tracks, pick the best ones, order them for the intended thematic arc, and explain each choice.

You MUST respond with ONLY valid JSON — no markdown fences, no explanation text, no preamble.`);
    claudeSessionReady = true;
  } catch {
    // Session already exists — that's fine
    claudeSessionReady = true;
  }
}

function parseResearchJson(text: string): { name: string; tracks: ResearchTrack[] } | null {
  const jsonMatch = text.match(/\{[\s\S]*\}/);
  if (!jsonMatch) return null;

  try {
    const parsed = JSON.parse(jsonMatch[0]);
    if (typeof parsed.name !== 'string' || !Array.isArray(parsed.tracks)) return null;
    const tracks: ResearchTrack[] = parsed.tracks
      .filter((t: any) => typeof t.id === 'number' && typeof t.title === 'string')
      .map((t: any) => ({
        id: t.id,
        title: t.title,
        artist: t.artist || 'Unknown',
        reason: t.reason || '',
      }));
    if (tracks.length === 0) return null;
    return { name: parsed.name, tracks };
  } catch {
    return null;
  }
}

function parseSearchSuggestions(text: string): string[] | null {
  const jsonMatch = text.match(/\{[\s\S]*\}/);
  if (!jsonMatch) return null;

  try {
    const parsed = JSON.parse(jsonMatch[0]);
    if (Array.isArray(parsed.searches) && parsed.searches.length > 0) {
      return parsed.searches.filter((s: any) => typeof s === 'string');
    }
    return null;
  } catch {
    return null;
  }
}

async function askClaude(prompt: string, allowedTools?: string[], onToolUse?: (toolName: string) => void): Promise<string> {
  let fullResponse = '';
  for await (const event of claudecode.chat(RESEARCH_SESSION, prompt, null, allowedTools)) {
    if (event.type === 'content') {
      fullResponse += event.text;
    } else if (event.type === 'tool_use' && onToolUse) {
      onToolUse(event.toolName);
    } else if (event.type === 'error') {
      throw new Error(event.message);
    }
  }
  return fullResponse;
}

async function researchPlaylist(query: string): Promise<void> {
  if (isResearching) return;
  isResearching = true;
  sparkleBtn.classList.add('researching');
  researchResult = null;

  try {
    await ensureClaudeSession();

    // ── Phase 1: AI Research via WebSearch ──
    setResearchStatus('Researching theme...');

    const researchPrompt = `Research this music theme/query and suggest specific search terms I can use to find matching tracks in a music catalog (Tidal).

Theme: "${query}"

Use WebSearch to find artists, albums, and tracks that match this theme. Look for:
- Artists known for this style/theme
- Specific albums or tracks that embody it
- Related genres and subgenres

Then return ONLY this JSON (no other text):
{"searches": ["artist name", "album title", "track title - artist", ...]}

Return 10-20 specific, varied search terms. Mix artist names, album titles, and "track - artist" pairs. Focus on terms that will actually find results in a streaming catalog.`;

    let searchSuggestions: string[] | null = null;

    try {
      let webSearchCount = 0;
      const researchResponse = await askClaude(researchPrompt, ['WebSearch'], (toolName) => {
        if (toolName === 'WebSearch') {
          webSearchCount++;
          setResearchStatus(webSearchCount === 1
            ? 'Searching the web...'
            : `Searching the web (${webSearchCount})...`);
        }
      });
      searchSuggestions = parseSearchSuggestions(researchResponse);
    } catch (err) {
      console.warn('Phase 1 research failed, falling back to direct search:', err);
    }

    // ── Phase 2: Search Tidal ──
    let allTracks: MonoEventSearchTrack[] = [];
    const seenTrackIds = new Set<number>();

    if (searchSuggestions && searchSuggestions.length > 0) {
      // AI-guided search: search for each suggestion in parallel
      const totalSearches = searchSuggestions.length;
      let completedSearches = 0;

      const searchPromises = searchSuggestions.map(async (term) => {
        const tracks: MonoEventSearchTrack[] = [];
        try {
          for await (const event of mono.search(term, 'tracks', 10)) {
            if (event.type === 'search_track') tracks.push(event as MonoEventSearchTrack);
          }
          // Also search albums for broader coverage
          for await (const event of mono.search(term, 'albums', 5)) {
            if (event.type === 'search_album') {
              // Search for tracks within found albums by album name + artist
              const album = event as MonoEventSearchAlbum;
              for await (const trackEvent of mono.search(`${album.title} ${album.artist}`, 'tracks', 5)) {
                if (trackEvent.type === 'search_track') tracks.push(trackEvent as MonoEventSearchTrack);
              }
            }
          }
        } catch {
          // Individual search failures are fine
        }
        completedSearches++;
        setResearchStatus(`Searching library (${completedSearches}/${totalSearches})...`);
        return tracks;
      });

      const results = await Promise.all(searchPromises);
      for (const tracks of results) {
        for (const track of tracks) {
          if (!seenTrackIds.has(track.id)) {
            seenTrackIds.add(track.id);
            allTracks.push(track);
          }
        }
      }
    }

    // Fallback: if AI research found nothing (or was skipped), do direct search
    if (allTracks.length === 0) {
      setResearchStatus('Searching tracks...');
      for await (const event of mono.search(query, 'tracks', 50)) {
        if (event.type === 'search_track') {
          const track = event as MonoEventSearchTrack;
          if (!seenTrackIds.has(track.id)) {
            seenTrackIds.add(track.id);
            allTracks.push(track);
          }
        }
      }
    }

    if (allTracks.length === 0) {
      setResearchStatus('No tracks found', true);
      setTimeout(() => setResearchStatus(''), 3000);
      isResearching = false;
      sparkleBtn.classList.remove('researching');
      return;
    }

    // ── Phase 3: Curate with Claude ──
    setResearchStatus(`Found ${allTracks.length} tracks, curating...`);

    const trackList = allTracks.map(t => ({
      id: t.id, title: t.title, artist: t.artist, album: t.album
    }));

    const curatePrompt = `Here are the tracks I found in our music library for the theme: "${query}"

Curate a playlist from these results. Pick the best tracks that fit the theme, order them to create a meaningful arc or journey, and explain each choice thematically.

Respond with ONLY this JSON (no other text):
{"name": "playlist name", "tracks": [{"id": 123, "title": "...", "artist": "...", "reason": "why this track fits the theme"}]}

Available tracks (${trackList.length} total):
${JSON.stringify(trackList)}`;

    let result: { name: string; tracks: ResearchTrack[] } | null = null;

    for (let attempt = 1; attempt <= MAX_RESEARCH_ATTEMPTS; attempt++) {
      setResearchStatus(attempt === 1
        ? `Found ${allTracks.length} tracks, curating...`
        : `Retrying (${attempt}/${MAX_RESEARCH_ATTEMPTS})...`);

      try {
        const prompt = attempt === 1
          ? curatePrompt
          : `Your previous response was not valid JSON. Please try again. Respond with ONLY a JSON object, nothing else:\n\n${curatePrompt}`;

        const response = await askClaude(prompt);
        result = parseResearchJson(response);

        if (result) break;

        console.warn(`Curation attempt ${attempt}: failed to parse JSON from response`);
      } catch (err) {
        console.error(`Curation attempt ${attempt} error:`, err);
        if (attempt === MAX_RESEARCH_ATTEMPTS) {
          setResearchStatus('Claude error — try again later', true);
          setTimeout(() => setResearchStatus(''), 5000);
        }
      }
    }

    if (result) {
      researchResult = result;

      // Auto-save: create playlist with Claude's reasoning as description
      const description = result.tracks
        .map(t => `${t.title} — ${t.artist}: ${t.reason}`)
        .join('\n');
      const trackIds = result.tracks.map(t => t.id);
      setResearchStatus('Saving playlist...');
      await rpcFire(playlist.create(result.name, description, trackIds));
      cachedPlaylists = null; // invalidate cache so list refreshes

      // Save research data (search suggestions + all found tracks before curation)
      const researchData = {
        query,
        searchSuggestions,
        allFoundTracks: allTracks.map(t => ({ id: t.id, title: t.title, artist: t.artist, album: t.album })),
        curatedTracks: result.tracks,
        createdAt: new Date().toISOString(),
      };
      rpcFire(playlist.researchSave(result.name, researchData));

      setResearchStatus('');
      // Auto-show the research results view
      showResearchResults();

      if (notificationsEnabled) {
        sendNotification({
          title: 'Playlist Research',
          body: `Saved: ${result.name} (${result.tracks.length} tracks)`
        });
      }
    } else if (!researchResult) {
      setResearchStatus('Could not parse results — try a different query', true);
      setTimeout(() => setResearchStatus(''), 5000);
    }
  } catch (err) {
    console.error('Research failed:', err);
    setResearchStatus('Research failed — check substrate connection', true);
    setTimeout(() => setResearchStatus(''), 5000);
  }

  isResearching = false;
  sparkleBtn.classList.remove('researching');
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

    const sub = makeTrackSub(track.artist);

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
      rpcFire(player.queueAdd(track.id, null, researchResult?.name));
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
    row.appendChild(makeDownloadBtn(track.id));
    row.appendChild(removeBtn);
    researchTracksEl.appendChild(row);
  }

  navigateTo('research');
}

// Research view buttons — playlist is auto-saved, this renames if user edited the name
researchCreateBtn.addEventListener('click', () => {
  if (!researchResult) return;
  const newName = researchNameInput.value.trim();
  if (newName && newName !== researchResult.name) {
    rpcFire(playlist.rename(researchResult.name, newName));
    cachedPlaylists = null;
  }
  researchResult = null;
  navigateTo('browse');
  loadPlaylists();
});

researchQueueBtn.addEventListener('click', () => {
  if (!researchResult) return;
  for (const track of researchResult.tracks) {
    rpcFire(player.queueAdd(track.id, null, researchResult.name));
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

// --- Like source helper ---
function getLikeSource(): string {
  if (currentView === 'detail' && currentPlaylistName) return `playlist:${currentPlaylistName}`;
  if (currentView === 'detail' && currentDetailAlbumId) return `album:${currentPlaylistName || 'unknown'}`;
  return currentView;
}

// --- Like button ---
likeBtn.addEventListener('click', () => {
  if (!currentTrackId) return;
  const wasLiked = likeBtn.classList.contains('liked');
  likeBtn.classList.toggle('liked');
  if (wasLiked) likedSet.delete(currentTrackId);
  else likedSet.add(currentTrackId);
  likeBtn.classList.remove('like-animate');
  void likeBtn.offsetWidth; // reflow to restart animation
  likeBtn.classList.add('like-animate');
  rpcFire(player.like(currentTrackId, getLikeSource()));
});
likeBtn.addEventListener('animationend', () => likeBtn.classList.remove('like-animate'));

// --- Download button (now-playing) — toggles download/delete/cancel ---
let activeDownload: AsyncGenerator<any> | null = null;

npDownloadBtn.addEventListener('click', async () => {
  if (!currentTrackId) return;
  if (npDownloadBtn.classList.contains('downloading') && activeDownload) {
    // Cancel in-progress download
    activeDownload.return(undefined);
    activeDownload = null;
    npDownloadBtn.classList.remove('downloading');
    npDownloadBtn.innerHTML = downloadIcon;
    rpcFire(player.deleteDownload(currentTrackId));
    return;
  }
  if (npDownloadBtn.classList.contains('downloaded')) {
    // Delete local file
    npDownloadBtn.classList.remove('downloaded');
    npDownloadBtn.innerHTML = downloadIcon;
    rpcFire(player.deleteDownload(currentTrackId));
  } else {
    // Download with progress ring
    const circumference = 2 * Math.PI * 10; // r=10
    npDownloadBtn.classList.add('downloading');
    npDownloadBtn.innerHTML = `<svg width="14" height="14" viewBox="0 0 24 24"><circle cx="12" cy="12" r="10" fill="none" stroke="var(--text-secondary)" stroke-width="2" opacity="0.2"/><circle class="dl-ring" cx="12" cy="12" r="10" fill="none" stroke="var(--accent)" stroke-width="2" stroke-linecap="round" stroke-dasharray="${circumference}" stroke-dashoffset="${circumference}" transform="rotate(-90 12 12)"/></svg>`;
    const ring = npDownloadBtn.querySelector('.dl-ring') as SVGCircleElement | null;
    const gen = player.downloadTrack(currentTrackId);
    activeDownload = gen;
    try {
      for await (const event of gen) {
        if (activeDownload !== gen) return; // cancelled
        if (event.type === 'download_progress' && ring) {
          const pct = (event as any).percent ?? 0;
          ring.style.strokeDashoffset = String(circumference * (1 - pct / 100));
        } else if (event.type === 'download_complete') {
          activeDownload = null;
          npDownloadBtn.classList.remove('downloading');
          npDownloadBtn.classList.add('downloaded');
          npDownloadBtn.innerHTML = checkIcon;
          return;
        }
      }
    } catch {
      npDownloadBtn.classList.remove('downloading');
      npDownloadBtn.innerHTML = downloadIcon;
    }
    activeDownload = null;
  }
});

// --- History button + view ---
historyBtn.addEventListener('click', () => {
  navigateTo('history');
  loadHistory();
});

async function loadHistory(): Promise<void> {
  historyTracksEl.innerHTML = '';
  historySubheader.textContent = 'Loading...';
  try {
    let tracks: QueuedTrack[] = [];
    for await (const event of player.historyList()) {
      if (event.type === 'queue') {
        tracks = (event as MonoEventQueue).tracks;
      }
    }
    // Reverse: most recent first
    tracks = tracks.slice().reverse();
    historySubheader.textContent = tracks.length > 0
      ? `${tracks.length} track${tracks.length !== 1 ? 's' : ''}`
      : '';
    historyTracksEl.innerHTML = '';
    if (tracks.length === 0) {
      const emptyEl = document.createElement('div');
      emptyEl.className = 'list-empty';
      emptyEl.textContent = 'No history yet';
      historyTracksEl.appendChild(emptyEl);
      return;
    }
    for (const track of tracks) {
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
      const sub = makeTrackSub(track.artist, { album: track.album, durationSecs: track.durationSecs, trackId: track.id });
      info.appendChild(titleSpan);
      info.appendChild(sub);
      // Like indicator
      if (likedSet.has(track.id)) {
        const heart = document.createElement('span');
        heart.className = 'like-indicator';
        heart.innerHTML = '<svg width="10" height="10" viewBox="0 0 24 24" fill="#e74c3c"><path d="M12 21.35l-1.45-1.32C5.4 15.36 2 12.28 2 8.5 2 5.42 4.42 3 7.5 3c1.74 0 3.41.81 4.5 2.09C13.09 3.81 14.76 3 16.5 3 19.58 3 22 5.42 22 8.5c0 3.78-3.4 6.86-8.55 11.54L12 21.35z"/></svg>';
        row.appendChild(heart);
      }
      row.appendChild(info);
      historyTracksEl.appendChild(row);
    }
  } catch {
    historySubheader.textContent = '';
    historyTracksEl.innerHTML = '';
    const errEl = document.createElement('div');
    errEl.className = 'list-empty';
    errEl.textContent = 'Failed to load history';
    historyTracksEl.appendChild(errEl);
  }
}

// --- Load liked track IDs at startup ---
async function loadLikedSet(): Promise<void> {
  try {
    for await (const event of player.likedTracks()) {
      if (event.type === 'queue') {
        const q = event as MonoEventQueue;
        likedSet.clear();
        for (const t of q.tracks) likedSet.add(t.id);
      }
    }
  } catch { /* not connected yet, will retry */ }
}

// --- Main loop: stream now_playing with reconnection ---
async function streamNowPlaying(): Promise<void> {
  while (true) {
    try {
      await rpc.connect();
      disconnectOverlay.classList.add('hidden');
      loadLikedSet(); // reload liked set on reconnect

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

// --- Audio peaks stream: ~30fps peak data for smooth waveform ---
async function streamAudioPeaks(): Promise<void> {
  // Seed waveform from server-side history buffer on connect
  try {
    for await (const event of player.waveform()) {
      if ((event as any).type === 'waveform') {
        const peaks = (event as any).peaks as number[];
        const start = Math.max(0, peaks.length - PEAK_BUFFER_SIZE);
        for (let i = start; i < peaks.length; i++) {
          peakBuffer[peakWriteIndex] = peaks[i];
          peakWriteIndex = (peakWriteIndex + 1) % PEAK_BUFFER_SIZE;
          if (peakBufferFilled < PEAK_BUFFER_SIZE) peakBufferFilled++;
        }
      }
    }
  } catch {
    // Waveform history not available, start empty
  }
  while (true) {
    try {
      for await (const event of player.audioPeaks()) {
        if ((event as any).type === 'audio_peak') {
          const peak = (event as any).peak as number;
          peakBuffer[peakWriteIndex] = peak;
          peakWriteIndex = (peakWriteIndex + 1) % PEAK_BUFFER_SIZE;
          if (peakBufferFilled < PEAK_BUFFER_SIZE) peakBufferFilled++;

          // Silence detection
          if (peak < 0.005) {
            if (silenceSince === null) silenceSince = Date.now();
          } else {
            silenceSince = null;
          }
        }
      }
    } catch {
      // Stream ended or disconnected, retry after delay
    }
    await new Promise(r => setTimeout(r, 2000));
  }
}
streamAudioPeaks();

// --- JS hover polyfill (CSS :hover doesn't fire in NSPanel WebView) ---
// Native global mouseMoved monitor in Rust emits coordinates via Tauri events.
// We use elementFromPoint to resolve the hovered element.
const hoverSelectors = '.nav-btn, .control-btn, .list-row, .row-action, #queue-btn, #open-link, .crumb, #progress-bar, .action-btn, .new-playlist-row, .save-queue-btn, .search-tab, .clickable-meta, #like-btn, #np-download-btn, #history-btn, .liked-songs-row';
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
const clickSelectors = '.nav-btn, .control-btn, .list-row, .row-action, #queue-btn, .action-btn, .new-playlist-row, .save-queue-btn, #like-btn, #np-download-btn, #history-btn, .liked-songs-row';
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
loadLikedSet();
