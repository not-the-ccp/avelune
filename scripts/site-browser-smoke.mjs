import fs from 'node:fs';
import http from 'node:http';
import os from 'node:os';
import path from 'node:path';
import {spawn} from 'node:child_process';

const root = path.resolve(process.argv[2] ?? 'dist/site');
const chromium = [process.env.CHROMIUM, '/usr/bin/chromium', '/usr/bin/chromium-browser', '/usr/bin/google-chrome'].find(candidate => candidate && fs.existsSync(candidate));
if (!chromium) { console.log('SKIP: Chromium is unavailable for rendered site smoke'); process.exit(0); }
if (!fs.existsSync(path.join(root, 'demo', 'demo.avl'))) throw Error('build the complete site before running the browser smoke');
function walk(directory) { return fs.readdirSync(directory,{withFileTypes:true}).flatMap(entry => entry.isDirectory() ? walk(path.join(directory,entry.name)) : [path.join(directory,entry.name)]); }
const pageRoutes = walk(root).filter(file => file.endsWith('.html') && !file.includes(`${path.sep}api${path.sep}rust${path.sep}`)).map(file => {
  const relative = path.relative(root,file).replaceAll(path.sep,'/');
  if (relative === 'index.html') return '/';
  return `/${relative.replace(/index\.html$/, '')}`;
});

const delay = ms => new Promise(resolve => setTimeout(resolve, ms));
const mime = new Map([['.html','text/html; charset=utf-8'],['.css','text/css'],['.js','text/javascript'],['.wasm','application/wasm'],['.avl','application/octet-stream'],['.svg','image/svg+xml'],['.woff2','font/woff2']]);
let jitterRangeResponses = 0;
const server = http.createServer(async (request, response) => {
  const url = new URL(request.url, 'http://localhost');
  if (!url.pathname.startsWith('/avelune/')) { response.writeHead(404).end(); return; }
  let relative = decodeURIComponent(url.pathname.slice('/avelune/'.length));
  if (!relative || relative.endsWith('/')) relative += 'index.html';
  const target = path.resolve(root, relative);
  if (!target.startsWith(`${root}${path.sep}`) || !fs.existsSync(target) || !fs.statSync(target).isFile()) { response.writeHead(404).end(); return; }
  const data = fs.readFileSync(target);
  const range = /^bytes=(\d+)-(\d+)$/.exec(request.headers.range ?? '');
  const headers = {'Content-Type': mime.get(path.extname(target)) ?? 'application/octet-stream', 'Accept-Ranges':'bytes'};
  if (range) {
    const first = Number(range[1]), last = Number(range[2]);
    if (first > last || last >= data.length) { response.writeHead(416).end(); return; }
    if (url.searchParams.get('jitter') === '1') {
      jitterRangeResponses++;
      // Let the fixed header/front-index reads complete normally, then inject a repeatable
      // transport stall into epoch-range delivery. Playback must consume its decode-ahead rather
      // than making the audio timeline depend directly on this response latency.
      if (jitterRangeResponses > 2) await delay(180);
    }
    response.writeHead(206, {...headers, 'Content-Length':last-first+1, 'Content-Range':`bytes ${first}-${last}/${data.length}`});
    response.end(data.subarray(first, last + 1)); return;
  }
  response.writeHead(200, {...headers, 'Content-Length':data.length}); response.end(data);
});
await new Promise(resolve => server.listen(0, '127.0.0.1', resolve));
const origin = `http://127.0.0.1:${server.address().port}`;
const profile = fs.mkdtempSync(path.join(os.tmpdir(), 'avelune-site-chrome-'));
const groupedProcess = process.platform !== 'win32';
// `--remote-debugging-port=0` makes Chromium publish its chosen ephemeral port in the profile;
// a guessed fixed port is what made the first CI run fail the DevTools connect. A cold or
// CPU-contended runner can take well over ten seconds to start the browser, so the poll window
// below is generous and proven at the CI scale.
const child = spawn(chromium, ['--headless=new','--no-sandbox','--disable-gpu','--disable-dev-shm-usage','--no-first-run','--no-default-browser-check','--disable-extensions','--autoplay-policy=no-user-gesture-required','--remote-debugging-port=0',`--user-data-dir=${profile}`,'about:blank'], {stdio:['ignore','ignore','pipe'],detached:groupedProcess});
// Keep Chromium diagnostics so a failed startup reports the actual error instead of a bare timeout.
let chromiumErrors = '';
child.stderr.on('data', chunk => { chromiumErrors = (chromiumErrors + chunk).slice(-8192); });

function chromiumFailure(prefix) {
  const exit = child.exitCode ?? child.signalCode ?? null;
  const detail = chromiumErrors.trim() || 'no stderr captured';
  return exit === null ? Error(`${prefix}: ${detail}`) : Error(`${prefix} (Chromium exited with ${exit}): ${detail}`);
}
const startupWindowAttempts = 600; // 600 * 50ms = 30s, matching Puppeteer's browser-launch budget.
async function devtoolsPort() {
  const portFile = path.join(profile, 'DevToolsActivePort');
  for (let attempt=0; attempt<startupWindowAttempts; attempt++) {
    if (child.exitCode !== null || child.signalCode !== null) break;
    try {
      const port = Number(fs.readFileSync(portFile, 'utf8').split('\n', 1)[0]);
      if (Number.isInteger(port) && port > 0) return port;
    } catch {}
    await delay(50);
  }
  throw chromiumFailure(`Chromium did not publish a DevTools port in ${portFile} within 30s`);
}
async function pageTarget() {
  const debugPort = await devtoolsPort();
  for (let attempt=0; attempt<startupWindowAttempts; attempt++) {
    if (child.exitCode !== null || child.signalCode !== null) break;
    try { const pages = await (await fetch(`http://127.0.0.1:${debugPort}/json/list`)).json(); const page = pages.find(item => item.type === 'page'); if (page) return page; } catch {}
    await delay(50);
  }
  throw chromiumFailure(`Chromium DevTools target unavailable on port ${debugPort} within 30s`);
}

let ws;
const pending = new Map();
let sequence = 0;
async function connect() {
  ws = new WebSocket((await pageTarget()).webSocketDebuggerUrl);
  await new Promise((resolve, reject) => { ws.onopen=resolve; ws.onerror=reject; });
  ws.onmessage = event => { const message=JSON.parse(event.data); const resolve=pending.get(message.id); if (resolve) { pending.delete(message.id); resolve(message); } };
}
const call = (method, params={}) => new Promise(resolve => { const id=++sequence; pending.set(id,resolve); ws.send(JSON.stringify({id,method,params})); });
async function evaluate(expression) {
  const reply = await call('Runtime.evaluate',{expression,awaitPromise:true,returnByValue:true});
  if (reply.result?.exceptionDetails) throw Error(reply.result.exceptionDetails.exception?.description ?? reply.result.exceptionDetails.text);
  return reply.result?.result?.value;
}
async function navigate(route) {
  await call('Page.navigate',{url:`${origin}/avelune${route}`});
  for (let attempt=0; attempt<200; attempt++) { if (await evaluate('document.readyState') === 'complete') return; await delay(50); }
  throw Error(`navigation timeout: ${route}`);
}
async function waitForDemoState(expected, attempts = 300, interval = 25) {
  const wanted = new Set(Array.isArray(expected) ? expected : [expected]);
  let state;
  for (let attempt=0; attempt<attempts; attempt++) {
    state = await evaluate("document.querySelector('#state-label')?.textContent");
    if (wanted.has(state) || state === 'ERROR') return state;
    await delay(interval);
  }
  return state;
}
async function auditPages(width, height) {
  await call('Emulation.setDeviceMetricsOverride',{width,height,deviceScaleFactor:1,mobile:width<600});
  for (const route of pageRoutes) {
    await navigate(route);
    const result = await evaluate("({h1:document.querySelectorAll('h1').length,overflow:document.documentElement.scrollWidth>innerWidth})");
    if (result.h1 !== 1 || result.overflow) throw Error(`${width}px page audit failed for ${route}: ${JSON.stringify(result)}`);
    const tree = await call('Accessibility.getFullAXTree');
    const unnamed = tree.result.nodes.filter(node => ['button','link','textbox','combobox','slider'].includes(node.role?.value) && !node.name?.value?.trim());
    if (unnamed.length) throw Error(`${width}px accessibility-name audit failed for ${route}: ${unnamed.map(node => node.role.value).join(', ')}`);
  }
}

try {
  await connect(); await call('Page.enable'); await call('Runtime.enable'); await call('Accessibility.enable');
  await navigate('/');
  await call('Input.dispatchKeyEvent',{type:'keyDown',key:'Tab',code:'Tab'});
  await call('Input.dispatchKeyEvent',{type:'keyUp',key:'Tab',code:'Tab'});
  if (!(await evaluate("document.activeElement?.classList.contains('skip-link')"))) throw Error('skip link is not the first keyboard focus target');
  await auditPages(1280,900);

  await navigate('/demo/');
  const mediaLayout = await evaluate(`(()=>{
    const panel=document.querySelector('.media-options');
    const fields=[...document.querySelectorAll('.media-field')];
    if(!panel||fields.length<8)return {missing:true};
    const bounds=panel.getBoundingClientRect();
    const rects=fields.map(field=>{const r=field.getBoundingClientRect();const control=field.querySelector('input,select')?.getBoundingClientRect();return {left:r.left,right:r.right,top:r.top,bottom:r.bottom,width:r.width,controlWidth:control?.width??0}});
    let overlaps=0;
    for(let i=0;i<rects.length;i++)for(let j=i+1;j<rects.length;j++){const a=rects[i],b=rects[j];if(Math.min(a.right,b.right)-Math.max(a.left,b.left)>1&&Math.min(a.bottom,b.bottom)-Math.max(a.top,b.top)>1)overlaps++;}
    return {missing:false,overlaps,outside:rects.some(r=>r.left<bounds.left-1||r.right>bounds.right+1),tooNarrow:rects.some(r=>r.width<120||r.controlWidth<100)};
  })()`);
  if (mediaLayout.missing || mediaLayout.overlaps || mediaLayout.outside || mediaLayout.tooNarrow) throw Error(`demo media-control layout is unusable at 1280px: ${JSON.stringify(mediaLayout)}`);
  await evaluate("(()=>{const option=document.querySelector('#sample').selectedOptions[0];option.value=option.value.split('?')[0]+'?jitter=1';document.querySelector('#load-sample').click()})()");
  let state = await waitForDemoState('READY', 300, 50);
  if (state !== 'READY') throw Error(`bundled demo did not become ready: ${await evaluate("document.querySelector('#state-detail')?.textContent")}`);
  if (await evaluate("document.querySelector('#play').disabled")) throw Error('play control remained disabled after sample load');

  await evaluate("document.querySelector('#play').click()");
  state = await waitForDemoState('PLAYING');
  if (state !== 'PLAYING') throw Error(`buffered demo did not start playback: ${await evaluate("document.querySelector('#state-detail')?.textContent")}`);
  for (let attempt=0; attempt<120; attempt++) {
    const t = Number(await evaluate("document.querySelector('#seek')?.value"));
    if (t > 0.25) break;
    if (attempt === 119) throw Error(`playback clock did not advance before pause (time=${t})`);
    await delay(25);
  }

  await evaluate("document.querySelector('#play').click()");
  state = await waitForDemoState('PAUSED', 80, 25);
  if (state !== 'PAUSED') throw Error(`pause did not settle cleanly: ${await evaluate("document.querySelector('#state-detail')?.textContent")}`);
  const pausedAt = Number(await evaluate("document.querySelector('#seek')?.value"));
  await delay(120);
  const pausedLater = Number(await evaluate("document.querySelector('#seek')?.value"));
  if (!(pausedAt > 0.2) || Math.abs(pausedLater - pausedAt) > 0.02) throw Error(`paused clock moved unexpectedly: ${pausedAt} -> ${pausedLater}`);

  await evaluate("document.querySelector('#play').click()");
  state = await waitForDemoState('PLAYING');
  if (state !== 'PLAYING') throw Error(`resume did not return to playback: ${await evaluate("document.querySelector('#state-detail')?.textContent")}`);

  const seekTarget = 1.35;
  await evaluate(`(()=>{const seek=document.querySelector('#seek');seek.value='${seekTarget}';seek.dispatchEvent(new Event('change',{bubbles:true}))})()`);
  state = await waitForDemoState('PLAYING', 300, 25);
  if (state !== 'PLAYING') throw Error(`seek while playing did not restart cleanly: ${await evaluate("document.querySelector('#state-detail')?.textContent")}`);
  const afterSeek = Number(await evaluate("document.querySelector('#seek')?.value"));
  if (afterSeek < seekTarget - 0.1) throw Error(`seek restarted before requested media time: ${afterSeek}`);

  state = await waitForDemoState('ENDED', 500, 25);
  const playback = await evaluate("({state:document.querySelector('#state-label')?.textContent,detail:document.querySelector('#state-detail')?.textContent,frames:Number(document.querySelector('#metric-frames')?.textContent||0),log:document.querySelector('#event-log')?.textContent||''})");
  if (state !== 'ENDED' || playback.frames <= 0 || !/0 audio underruns/.test(playback.detail) || /playback error:/i.test(playback.log)) {
    throw Error(`buffer-driven demo playback failed under range jitter/interactions: ${JSON.stringify(playback)}`);
  }
  if (jitterRangeResponses < 3) throw Error(`playback jitter regression did not exercise delayed epoch ranges (${jitterRangeResponses} range responses)`);

  await evaluate("document.querySelector('#play').click()");
  state = await waitForDemoState('PLAYING', 300, 25);
  if (state !== 'PLAYING') throw Error(`replay did not restart from ended state: ${await evaluate("document.querySelector('#state-detail')?.textContent")}`);
  const replayAt = Number(await evaluate("document.querySelector('#seek')?.value"));
  if (replayAt > 0.25) throw Error(`replay did not restart near the beginning: ${replayAt}`);
  await evaluate("document.querySelector('#play').click()");
  state = await waitForDemoState('PAUSED', 80, 25);
  if (state !== 'PAUSED') throw Error('replay pause cleanup failed');

  await call('Emulation.setDeviceMetricsOverride',{width:320,height:800,deviceScaleFactor:1,mobile:true});
  await call('Emulation.setEmulatedMedia',{media:'screen',features:[{name:'prefers-color-scheme',value:'light'}]});
  await navigate('/spec/video/alv1/');
  const mobile = await evaluate("({overflow:document.documentElement.scrollWidth>innerWidth,toc:getComputedStyle(document.querySelector('.mobile-toc')).display,nav:!!document.querySelector('.section-nav details')})");
  if (mobile.overflow || mobile.toc === 'none' || !mobile.nav) throw Error(`mobile publication navigation/reflow failed: ${JSON.stringify(mobile)}`);
  if (await evaluate("getComputedStyle(document.body).backgroundColor") !== 'rgb(243, 241, 234)') throw Error('publication light palette was not applied');
  await call('Emulation.setEmulatedMedia',{media:'screen',features:[{name:'prefers-color-scheme',value:'dark'}]});
  await auditPages(320,800);
  if (await evaluate("getComputedStyle(document.body).backgroundColor") !== 'rgb(20, 21, 19)') throw Error('publication dark palette was not applied');
  await call('Emulation.setEmulatedMedia',{media:'print'});
  if (await evaluate("getComputedStyle(document.querySelector('.site-header')).display") !== 'none') throw Error('publication chrome remains visible in print');
  await call('Emulation.setEmulatedMedia',{media:'screen',features:[{name:'prefers-color-scheme',value:'dark'}]});

  await navigate('/search/');
  // Filtering must actually remove unmatched rows from rendering, not only reflect the attribute.
  const filtered = await evaluate("(()=>{const input=document.querySelector('#site-search');input.value='avelune-no-such-term-7q9z';input.dispatchEvent(new Event('input'));const rows=[...document.querySelectorAll('#search-results>li')];const hidden=rows.filter(x=>x.hidden);return {total:rows.length,hidden:hidden.length,renderedHidden:hidden.every(x=>getComputedStyle(x).display==='none')}})()");
  if (!filtered.total || filtered.hidden !== filtered.total || !filtered.renderedHidden) throw Error(`site search filtering failed: ${JSON.stringify(filtered)}`);
  const count = await evaluate("(()=>{const i=document.querySelector('#site-search');i.value='epoch';i.dispatchEvent(new Event('input'));return [...document.querySelectorAll('#search-results>li')].filter(x=>!x.hidden).length})()");
  if (!count) throw Error('site search returned no epoch documents');
  console.log(JSON.stringify({pages:pageRoutes.length,widths:[1280,320],keyboard:'skip-link',demo:'buffered-jitter-pause-seek-replay',jitterRanges:jitterRangeResponses,mobilePublication:'reflowed',themes:'light/dark/print',searchResults:count}));
} finally {
  ws?.close();
  const exited = new Promise(resolve => child.once('exit', resolve));
  if (groupedProcess) process.kill(-child.pid, 'SIGTERM'); else child.kill('SIGTERM');
  await Promise.race([exited, delay(2000)]);
  await new Promise(resolve => server.close(resolve));
  for (let attempt=0; attempt<10; attempt++) {
    try { fs.rmSync(profile,{recursive:true,force:true,maxRetries:2,retryDelay:100}); break; }
    catch (error) { if (attempt === 9) throw error; await delay(200); }
  }
}
