import { defineConfig } from '@playwright/test';

export default defineConfig({
  testDir: './tests',
  timeout: 60_000,
  use: {
    viewport: { width: 352, height: 600 },
    colorScheme: 'dark',
  },
  webServer: {
    command: 'bun run build && bunx serve dist -l 5199 --no-clipboard',
    port: 5199,
    reuseExistingServer: true,
    timeout: 15_000,
  },
});
