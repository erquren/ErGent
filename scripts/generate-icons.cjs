// Regenerate browser assets from the original artwork: node scripts/generate-icons.cjs
const { chromium } = require("@playwright/test");
const fs = require("node:fs/promises");
const path = require("node:path");
const root = path.resolve(__dirname, "..");
(async () => {
  const dir = path.join(root, "apps/web/public");
  await fs.mkdir(path.join(dir, "branding"), { recursive: true });
  const mark = (
    await fs.readFile(path.join(root, "ErGent_logo_mark.svg"), "utf8")
  ).replace('viewBox="0 0 512 512"', 'viewBox="64 8 400 400"');
  const word = await fs.readFile(
    path.join(root, "ErGent_wordmark.svg"),
    "utf8",
  );
  await fs.writeFile(path.join(dir, "branding/logo.svg"), mark);
  await fs.writeFile(path.join(dir, "favicon.svg"), mark);
  const browser = await chromium.launch({ headless: true });
  try {
    const page = await browser.newPage({ deviceScaleFactor: 1 });
    async function png(svg, size, background = "transparent", inset = 0) {
      await page.setViewportSize({ width: size, height: size });
      await page.setContent(
        `<style>html,body{margin:0;width:100%;height:100%;background:${background}}svg{display:block;width:${size - inset * 2}px;height:${size - inset * 2}px;margin:${inset}px}</style>${svg}`,
      );
      return await page.screenshot({
        omitBackground: background === "transparent",
      });
    }
    const images = [];
    for (const size of [16, 32, 48, 192, 512]) {
      const data = await png(mark, size);
      await fs.writeFile(path.join(dir, `icon-${size}.png`), data);
      if (size <= 48) images.push({ size, data });
    }
    // ICO container with three lossless PNG entries for legacy desktop browsers.
    const header = Buffer.alloc(6 + 16 * images.length);
    header.writeUInt16LE(1, 2);
    header.writeUInt16LE(images.length, 4);
    let offset = header.length;
    images.forEach(({ size, data }, i) => {
      const p = 6 + 16 * i;
      header[p] = size;
      header[p + 1] = size;
      header.writeUInt16LE(1, p + 4);
      header.writeUInt16LE(32, p + 6);
      header.writeUInt32LE(data.length, p + 8);
      header.writeUInt32LE(offset, p + 12);
      offset += data.length;
    });
    await fs.writeFile(
      path.join(dir, "favicon.ico"),
      Buffer.concat([header, ...images.map((i) => i.data)]),
    );
    await fs.writeFile(
      path.join(dir, "apple-touch-icon.png"),
      await png(mark, 180, "#14191f", 8),
    );
    await fs.writeFile(
      path.join(dir, "icon-maskable-512.png"),
      await png(mark, 512, "#14191f", 64),
    );
    await page.setViewportSize({ width: 1100, height: 300 });
    await page.setContent(`<style>body{margin:0}</style>${word}`);
    await page.evaluate(() => document.fonts.ready);
    const box = await page.locator("svg > g").evaluate((el) => {
      const b = el.getBBox();
      return {
        x: b.x - 30,
        y: b.y - 30,
        width: b.width + 60,
        height: b.height + 60,
      };
    });
    const cropped = word.replace(
      'viewBox="0 0 1100 300"',
      `viewBox="${box.x} ${box.y} ${box.width} ${box.height}"`,
    );
    await fs.writeFile(path.join(dir, "branding/wordmark.svg"), cropped);
    const width = 960,
      height = Math.ceil((width * box.height) / box.width);
    await page.setViewportSize({ width, height });
    await page.setContent(
      `<style>body{margin:0}svg{width:100%;height:100%;display:block}</style>${cropped}`,
    );
    await fs.writeFile(
      path.join(dir, "branding/wordmark.png"),
      await page.screenshot({ omitBackground: true }),
    );
    console.log("Generated SVG, PNG, ICO, Apple touch and maskable icons.");
  } finally {
    await browser.close();
  }
})().catch((e) => {
  console.error(e);
  process.exitCode = 1;
});
