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

const mime = new Map([['.html','text/html; charset=utf-8'],['.css','text/css'],['.js','text/javascript'],['.wasm','application/wasm'],['.avl','application/octet-stream'],['.svg','image/svg+xml'],['.woff2','font/woff2']]);
const server = http.createServer((request, response) => {
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
    response.writeHead(206, {...headers, 'Content-Length':last-first+1, 'Content-Range':`bytes ${first}-${last}/${data.length}`});
    response.end(data.subarray(first, last + 1)); return;
  }
  response.writeHead(200, {...headers, 'Content-Length':data.length}); response.end(data);
});
await new Promise(resolve => server.listen(0, '127.0.0.1', resolve));
const origin = `http://127.0.0.1:${server.address().port}`;
const debugPort = 20000 + process.pid % 10000;
const profile = fs.mkdtempSync(path.join(os.tmpdir(), 'avelune-site-chrome-'));
const groupedProcess = process.platform !== 'win32';
const child = spawn(chromium, ['--headless=new','--no-sandbox','--disable-gpu','--disable-dev-shm-usage',`--remote-debugging-port=${debugPort}`,`--user-data-dir=${profile}`,'about:blank'], {stdio:'ignore',detached:groupedProcess});

const delay = ms => new Promise(resolve => setTimeout(resolve, ms));
async function pageTarget() {
  for (let attempt=0; attempt<200; attempt++) {
    try { const pages = await (await fetch(`http://127.0.0.1:${debugPort}/json/list`)).json(); const page = pages.find(item => item.type === 'page'); if (page) return page; } catch {}
    await delay(50);
  }
  throw Error('Chromium DevTools target unavailable');
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
  await evaluate("document.querySelector('#load-sample').click()");
  let state;
  for (let attempt=0; attempt<300; attempt++) { state=await evaluate("document.querySelector('#state-label')?.textContent"); if (state === 'READY' || state === 'ERROR') break; await delay(50); }
  if (state !== 'READY') throw Error(`bundled demo did not become ready: ${await evaluate("document.querySelector('#state-detail')?.textContent")}`);
  if (await evaluate("document.querySelector('#play').disabled")) throw Error('play control remained disabled after sample load');

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
  const count = await evaluate("(()=>{const i=document.querySelector('#site-search');i.value='epoch';i.dispatchEvent(new Event('input'));return [...document.querySelectorAll('#search-results>li')].filter(x=>!x.hidden).length})()");
  if (!count) throw Error('site search returned no epoch documents');
  console.log(JSON.stringify({pages:pageRoutes.length,widths:[1280,320],keyboard:'skip-link',demo:'ready',mobilePublication:'reflowed',themes:'light/dark/print',searchResults:count}));
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
