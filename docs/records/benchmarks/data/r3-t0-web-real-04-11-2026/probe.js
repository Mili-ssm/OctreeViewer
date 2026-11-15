async page => {
  await page.addInitScript(() => {
    window.__r3events = [];
    const OriginalWorker = window.Worker;
    window.Worker = class extends OriginalWorker {
      constructor(...args) {
        super(...args);
        this.addEventListener('message', ({data}) => {
          if (data?.lod || data?.opened) window.__r3events.push({type: data.type, opened: data.opened, lod: data.lod});
        });
      }
    };
  });
  const results = [];
  for (const name of ['crs-root0', 'crs', 'local']) for (const version of ['v74', 'v75']) {
    const stem = `${name}-${version}`;
    await page.goto('http://127.0.0.1:18777/examples/web-host/index.html');
    await page.waitForFunction(() => !document.querySelector('#dataset-file').disabled, null, {timeout: 30000});
    await page.locator('#attributes-file').setInputFiles(`target/r3-fixtures/${stem}.octattrs`);
    await page.locator('#dataset-file').setInputFiles(`target/r3-fixtures/${stem}.oct`);
    await page.waitForFunction(s => document.body.innerText.includes(`dataset: ${s}.oct`) && document.body.innerText.includes('target reached'), stem, {timeout: 30000});
    const result = await page.evaluate(() => ({userAgent: navigator.userAgent, text: document.body.innerText, events: window.__r3events.map(e => ({type:e.type, paged:e.opened?.paged, nodeCount:e.opened?.nodeCount, pending:e.lod?.pendingPageAnchors, activity:e.lod?.pageActivity})).filter(e => e.paged !== undefined || e.activity)}));
    const opened = result.events.find(e => e.type === 'datasetOpened');
    if (!opened || opened.paged !== (version === 'v75') || opened.nodeCount !== 3 || !result.text.includes('deepest visible LOD level: 2') || !result.text.includes('LOD coverage: complete (0 uncovered)')) throw new Error(`${stem}: opening/coverage failed`);
    if (version === 'v75') {
      const before = result.events.findIndex(e => e.activity?.residentNodes === 2 && e.pending?.includes('1'));
      const after = result.events.findIndex((e, i) => i > before && e.activity?.residentNodes === 3 && e.activity?.residentPages === 1 && e.pending?.length === 0);
      if (before < 0 || after < 0) throw new Error(`${stem}: missing deferred graft transition`);
    } else if (!result.events.some(e => e.activity?.residentNodes === 3 && e.activity?.residentPages === 0)) throw new Error(`${stem}: missing full index`);
    results.push({stem, userAgent:result.userAgent, lines:result.text.split('\n').filter(x => /dataset:|deepest visible|coverage:|pending blocks:|visible blocks:|target reached/.test(x)), events:result.events});
  }
  return results;
}
