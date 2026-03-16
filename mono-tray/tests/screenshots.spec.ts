import { test, expect } from '@playwright/test';

// Mock Tauri internals so the app runs in a plain browser
const TAURI_MOCK = `
  window.__TAURI__ = {};
  window.__TAURI_INTERNALS__ = {
    metadata: {
      currentWindow: { label: 'main' },
      currentWebview: { label: 'main', windowLabel: 'main' },
    },
    invoke: async (cmd, args) => {
      if (cmd === 'plugin:window|set_size') return;
      if (cmd === 'plugin:window|hide') return;
      if (cmd === 'plugin:window|show') return;
      if (cmd === 'plugin:window|is_focused') return false;
      if (cmd === 'plugin:notification|is_permission_granted') return true;
      if (cmd === 'plugin:notification|request_permission') return 'granted';
      if (cmd === 'plugin:notification|notify') return;
      if (cmd === 'plugin:event|listen') return 0;
      if (cmd === 'plugin:event|unlisten') return;
      if (cmd === 'plugin:resources|close') return;
      return null;
    },
    transformCallback: (cb, once) => {
      const id = Math.floor(Math.random() * 1e9);
      window['_' + id] = cb;
      return id;
    },
    convertFileSrc: (path) => path,
    unregisterCallback: () => {},
  };
`;

const FORCE_VISIBLE_CSS = `
  html, body { background: #111 !important; }
  #app {
    opacity: 1 !important;
    transform: none !important;
    transition: none !important;
  }
`;

const SCREENSHOT_DIR = '../docs/screenshots';

test.describe('Mono Tray Screenshots', () => {
  test.beforeEach(async ({ page }) => {
    await page.addInitScript(TAURI_MOCK);
    page.on('console', msg => {
      if (msg.type() === 'error') console.log(`[BROWSER ERROR] ${msg.text()}`);
    });
    page.on('pageerror', err => console.log(`[PAGE ERROR] ${err.message}`));
  });

  async function setup(page: any, height: number) {
    await page.setViewportSize({ width: 352, height });
    await page.goto('http://localhost:5199');
    await page.addStyleTag({ content: FORCE_VISIBLE_CSS });
  }

  test('now-playing view', async ({ page }) => {
    await setup(page, 582);
    // Wait for track title to populate (not "Not Playing")
    await page.locator('#title').filter({ hasNotText: 'Not Playing' }).waitFor({ timeout: 10000 });
    // Wait for cover art to load (best-effort — upstream API may be slow)
    await page.locator('#album-art[src]:not([src=""])').waitFor({ timeout: 15000 }).catch(() => {});
    await page.waitForTimeout(500);
    await page.screenshot({ path: `${SCREENSHOT_DIR}/now-playing.png` });
  });

  test('browse view with playlists', async ({ page }) => {
    await setup(page, 600);
    // Wait for now-playing data first (confirms WS is connected)
    await page.locator('#title').filter({ hasNotText: 'Not Playing' }).waitFor({ timeout: 10000 });

    await page.click('#nav-action');
    // Wait for playlist rows to appear in the browse list
    await page.locator('#browse-list .list-row').first().waitFor({ timeout: 10000 });
    await page.waitForTimeout(300);
    await page.screenshot({ path: `${SCREENSHOT_DIR}/browse.png` });
  });

  test('search results', async ({ page }) => {
    await setup(page, 600);
    await page.locator('#title').filter({ hasNotText: 'Not Playing' }).waitFor({ timeout: 10000 });

    await page.click('#nav-action');
    await page.locator('#browse-list .list-row').first().waitFor({ timeout: 10000 });

    // Type search query
    await page.click('#search-input');
    await page.keyboard.type('radiohead', { delay: 50 });
    // Wait for actual search results — a row containing "Radiohead" (case-insensitive)
    await page.locator('#browse-list .list-row .list-row-sub').filter({ hasText: /radiohead/i }).first().waitFor({ timeout: 30000 }).catch(() => {});
    // Give results a moment to fully render
    await page.waitForTimeout(500);
    await page.screenshot({ path: `${SCREENSHOT_DIR}/search.png` });
  });

  test('queue view', async ({ page }) => {
    await setup(page, 600);
    await page.locator('#title').filter({ hasNotText: 'Not Playing' }).waitFor({ timeout: 10000 });

    await page.click('#queue-btn');
    // Wait for queue track rows to appear
    await page.locator('#queue-tracks .list-row').first().waitFor({ timeout: 10000 });
    await page.waitForTimeout(300);
    await page.screenshot({ path: `${SCREENSHOT_DIR}/queue.png` });
  });

  test('history view', async ({ page }) => {
    await setup(page, 600);
    await page.locator('#title').filter({ hasNotText: 'Not Playing' }).waitFor({ timeout: 10000 });

    await page.click('#history-btn');
    // Wait for history track rows to appear
    await page.locator('#history-tracks .list-row').first().waitFor({ timeout: 10000 });
    await page.waitForTimeout(300);
    await page.screenshot({ path: `${SCREENSHOT_DIR}/history.png` });
  });

  test('playlist detail', async ({ page }) => {
    await setup(page, 600);
    await page.locator('#title').filter({ hasNotText: 'Not Playing' }).waitFor({ timeout: 10000 });

    await page.click('#nav-action');
    // Wait for playlist rows
    const playlistRow = page.locator('#browse-list .list-row').first();
    await playlistRow.waitFor({ timeout: 10000 });

    await playlistRow.click();
    // Wait for detail tracks to load
    await page.locator('#detail-tracks .list-row').first().waitFor({ timeout: 10000 });
    await page.waitForTimeout(300);
    await page.screenshot({ path: `${SCREENSHOT_DIR}/playlist-detail.png` });
  });
});
