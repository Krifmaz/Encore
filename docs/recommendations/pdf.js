// Prints recommendations.html (argv[2], absolute path) to an A4 PDF (argv[3]) with page numbers, using Playwright.
const { chromium } = require('playwright');
(async () => {
  const b = await chromium.launch(); const p = await b.newPage();
  await p.goto('file://' + process.argv[2]); await p.waitForTimeout(300);
  await p.pdf({ path: process.argv[3], format: 'A4', printBackground: true, displayHeaderFooter: true,
    headerTemplate: '<span></span>',
    footerTemplate: '<div style="font-size:8px;color:#888;width:100%;text-align:center;font-family:sans-serif">How Encore recommends music · <span class="pageNumber"></span> / <span class="totalPages"></span></div>',
    margin: { top: '14mm', bottom: '16mm', left: '15mm', right: '15mm' } });
  await b.close();
})();
