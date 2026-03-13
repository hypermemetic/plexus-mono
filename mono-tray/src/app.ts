import { PlexusRpcClient } from '../generated/transport';
import { createPlayerClient } from '../generated/player/client';
import { createMonoClient } from '../generated/mono/client';
import type { MonoEventNowPlaying, MonoEventCover } from '../generated/player/types';
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

// --- State ---
let currentTrackId: number | null = null;
let isPlaying = false;
let volumeDebounce: ReturnType<typeof setTimeout> | null = null;
let notificationsEnabled = false;

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
const mono = createMonoClient(rpc);

// --- Helpers ---
function formatTime(secs: number): string {
  const m = Math.floor(secs / 60);
  const s = Math.floor(secs % 60);
  return `${m}:${s.toString().padStart(2, '0')}`;
}

async function fetchCoverArt(trackId: number): Promise<void> {
  try {
    for await (const event of mono.cover(trackId, 320)) {
      if (event.type === 'cover') {
        const cover = event as MonoEventCover;
        albumArt.src = cover.url;
        albumArt.classList.add('loaded');
        return;
      }
    }
  } catch {
    // Cover not available
    albumArt.classList.remove('loaded');
  }
}

function updatePlayPauseIcon(status: string): void {
  isPlaying = status === 'playing' || status === 'buffering' || status === 'starting';
  iconPlay.classList.toggle('hidden', isPlaying);
  iconPause.classList.toggle('hidden', !isPlaying);
}

function updateUI(np: MonoEventNowPlaying): void {
  // Track info
  titleEl.textContent = np.title || 'Not Playing';

  const parts: string[] = [];
  if (np.artist) parts.push(np.artist);
  if (np.album) parts.push(np.album);
  artistAlbumEl.textContent = parts.join(' — ');

  // Progress
  const pct = np.durationSecs > 0 ? (np.positionSecs / np.durationSecs) * 100 : 0;
  progressFill.style.width = `${pct}%`;
  progressThumb.style.left = `${pct}%`;
  timeCurrent.textContent = formatTime(np.positionSecs);
  timeTotal.textContent = formatTime(np.durationSecs);

  // Play/pause icon
  updatePlayPauseIcon(np.status);

  // Volume (don't update while user is dragging)
  if (!volumeSlider.matches(':active')) {
    volumeSlider.value = String(Math.round(np.volume * 100));
  }

  // Queue
  if (np.queueLength > 0) {
    queueInfo.textContent = `${np.queueLength} in queue`;
  } else {
    queueInfo.textContent = '';
  }

  // Open link
  if (np.trackId) {
    openLink.style.display = '';
    openLink.dataset.url = `https://monochrome.tf/track/t/${np.trackId}`;
  } else {
    openLink.style.display = 'none';
  }

  // Fetch cover art and notify when track changes
  if (np.trackId && np.trackId !== currentTrackId) {
    currentTrackId = np.trackId;
    fetchCoverArt(np.trackId);
    notifyTrackChange(np);
  } else if (!np.trackId) {
    currentTrackId = null;
    albumArt.classList.remove('loaded');
  }
}

// --- Transport controls ---
btnPlayPause.addEventListener('click', async () => {
  try {
    const gen = isPlaying ? player.pause() : player.resume();
    // Consume generator (fire-and-forget)
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
    await new Promise(r => setTimeout(r, 2000));
  }
}

streamNowPlaying();
