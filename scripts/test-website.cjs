'use strict';
// Local/static and published-site verification. Uses an existing browser only.
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const { pathToFileURL } = require('node:url');
const { chromium } = require(process.env.PLAYWRIGHT_MODULE || 'playwright');
const out = path.resolve(process.argv[2]);
if (fs.existsSync(out)) throw new Error('Use a new evidence directory.');
fs.mkdirSync(out, { recursive: true });
const target = process.env.SITE_URL || pathToFileURL(path.resolve('web/index.html')).href;
(async () => {
  const browser = await chromium.launch({ headless: true, executablePath: process.env.SHOWCASE_BROWSER || 'C:\\Program Files (x86)\\Microsoft\\Edge\\Application\\msedge.exe' });
  const report = { target, cases: [] };
  try {
    for (const width of [360, 768, 1440]) {
      const page = await browser.newPage({ viewport: { width, height: 900 }, deviceScaleFactor: 1 });
      const errors = [];
      page.on('pageerror', e => errors.push(e.message));
      await page.addInitScript(() => {
        window.testCLS = 0;
        new PerformanceObserver(list => { for (const e of list.getEntries()) if (!e.hadRecentInput) window.testCLS += e.value; }).observe({ type: 'layout-shift', buffered: true });
      });
      await page.goto(target, { waitUntil: 'networkidle' });
      await page.locator('.native-figure').scrollIntoViewIfNeeded();
      await page.waitForFunction(() => [...document.images].every(i => i.complete && i.naturalWidth > 0));
      const seo = await page.evaluate(() => ({
        title: document.title,
        description: document.querySelector('meta[name="description"]').content,
        canonical: document.querySelector('link[rel="canonical"]').href,
        heading: [...document.querySelectorAll('h1')].map(e => e.textContent),
        schema: JSON.parse(document.querySelector('script[type="application/ld+json"]').textContent),
        overflow: document.documentElement.scrollWidth > innerWidth,
        overflowing: [...document.querySelectorAll('body *')].map(e=>({tag:e.tagName,class:e.className,right:e.getBoundingClientRect().right})).filter(e=>e.right>innerWidth+1).slice(0,20),
        brokenAnchors: [...document.querySelectorAll('a[href^="#"]')].filter(e => !document.querySelector(e.getAttribute('href'))).length,
        imagesNamed: [...document.images].every(e => e.hasAttribute('alt') && e.width > 0 && e.height > 0),
        cls: window.testCLS,
        download: [...document.querySelectorAll('a[download]')].map(e => e.href),
      }));
      report.latest = { width, seo };
      assert.match(seo.title, /^AI 바탕화면 위젯 캘린더/);
      assert.equal(seo.heading.length, 1);
      assert.equal(seo.heading[0].replace(/\s/g, ''), 'AI바탕화면위젯캘린더');
      assert.equal(seo.canonical, 'https://7065616b.github.io/dot-calendar/');
      assert.equal(seo.schema['@type'], 'SoftwareApplication');
      assert.equal(seo.schema.softwareVersion, '0.3.18');
      assert.equal(seo.schema.name, 'AI 캘린더');
      assert.ok(!seo.schema.aggregateRating && !seo.schema.review);
      assert.ok(seo.description.length > 70 && seo.description.length < 200);
      assert.equal(seo.overflow, false);
      assert.equal(seo.brokenAnchors, 0);
      assert.equal(seo.imagesNamed, true);
      assert.ok(seo.cls < .01, `Unexpected load layout shift ${seo.cls}`);
      assert.ok(seo.download.every(url => url.startsWith('https://github.com/7065616b/dot-calendar/releases/download/v0.3.18/')));
      await page.locator('#calendar-demo').scrollIntoViewIfNeeded();
      await page.locator('.day-select').filter({ hasText: /^8$/ }).click();
      assert.match(await page.locator('#day-count').innerText(), /5/);
      await page.locator('#opacity').fill('60');
      assert.equal(await page.locator('#opacity-value').innerText(), '60%');
      await page.locator('summary').first().click();
      assert.equal(await page.locator('details').first().getAttribute('open'), '');
      await page.keyboard.press('Control+Home');
      await page.screenshot({ path: path.join(out, `site-${width}.png`), fullPage: true });
      await page.emulateMedia({ reducedMotion: 'reduce' });
      assert.equal(await page.locator('.button').first().evaluate(e => getComputedStyle(e).transitionDuration), '1e-05s');
      assert.deepEqual(errors, []);
      report.cases.push({ width, status: 'PASS', seo, errors });
      await page.close();
    }
    report.status = 'PASS';
  } catch (error) {
    report.status = 'FAIL'; report.error = error.stack; process.exitCode = 1;
  } finally {
    await browser.close();
    fs.writeFileSync(path.join(out, 'result.json'), JSON.stringify(report, null, 2));
    console.log(JSON.stringify({ status: report.status, cases: report.cases.length, error: report.error }));
  }
})();
