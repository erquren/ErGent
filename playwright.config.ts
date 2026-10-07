import { defineConfig } from "@playwright/test";
export default defineConfig({
  testDir: "tests/e2e",
  timeout: 90000,
  workers: 1,
  fullyParallel: false,
  use: {
    locale: "zh-CN",
    viewport: { width: 1440, height: 960 },
    screenshot: "only-on-failure",
    trace: "retain-on-failure",
  },
  reporter: "list",
});
