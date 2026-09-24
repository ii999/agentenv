// Recording-detection probe for the CDP filling lab.
//
// Question: when a filling client attaches to an existing Chromium-family
// browser as a second CDP client, which recorders started by another client
// can it observe WITHOUT starting, stopping, or otherwise mutating them?
//
// Roles:
//   host     - playwright-core connectOverCDP; starts and stops each recorder.
//   observer - an independent WebSocket to the browser endpoint; issues only
//              read-only queries and compares snapshots taken before, during
//              and after each recorder.
//
// The observer never calls a start/stop/end method. In particular, probing
// tracing by calling Tracing.start and looking for an "already started" error
// is deliberately excluded because it starts a trace.
//
// Usage: node probe.mjs [--output-dir DIR] [--browser PATH]

import { spawn } from 'node:child_process';
import { mkdtemp, mkdir, readFile, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { setTimeout as sleep } from 'node:timers/promises';

import { chromium } from 'playwright-core';

const args = process.argv.slice(2);
const argValue = (name, fallback) => {
  const index = args.indexOf(name);
  return index === -1 ? fallback : args[index + 1];
};
const BROWSER = argValue('--browser', '/Applications/Microsoft Edge.app/Contents/MacOS/Microsoft Edge');
const OUTPUT_DIR = argValue('--output-dir', '.dev/artifacts/work/credential-fill/scratch/recording-dev');
const HISTOGRAM_PATTERN = /trac|screen|devtools|screenshot|capture|video|record|perfetto/i;

// ---------------------------------------------------------------- browser

async function launchBrowser(extraArgs = []) {
  const profile = await mkdtemp(join(tmpdir(), 'cdp-recording-probe-'));
  const proc = spawn(
    BROWSER,
    [
      '--remote-debugging-port=0',
      `--user-data-dir=${profile}`,
      '--no-first-run',
      '--no-default-browser-check',
      '--disable-sync',
      '--window-size=900,700',
      ...extraArgs,
      'about:blank',
    ],
    { stdio: ['ignore', 'ignore', 'ignore'] },
  );
  const portFile = join(profile, 'DevToolsActivePort');
  let port = 0;
  let path = '';
  for (let attempt = 0; attempt < 200; attempt += 1) {
    try {
      const text = await readFile(portFile, 'utf8');
      const lines = text.trim().split('\n');
      if (lines.length >= 2 && Number(lines[0]) > 0) {
        port = Number(lines[0]);
        path = lines[1];
        break;
      }
    } catch {
      // not written yet
    }
    await sleep(100);
  }
  if (!port) throw new Error('browser did not publish DevToolsActivePort');
  return {
    proc,
    profile,
    httpBase: `http://127.0.0.1:${port}`,
    wsUrl: `ws://127.0.0.1:${port}${path}`,
    async close() {
      proc.kill('SIGTERM');
      await Promise.race([new Promise((resolve) => proc.once('exit', resolve)), sleep(5000)]);
      if (proc.exitCode === null) proc.kill('SIGKILL');
      await rm(profile, { recursive: true, force: true });
    },
  };
}

// --------------------------------------------------------------- observer

class Observer {
  constructor(wsUrl) {
    this.wsUrl = wsUrl;
    this.nextId = 1;
    this.pending = new Map();
    this.events = [];
    this.calls = [];
  }

  async connect() {
    this.ws = new WebSocket(this.wsUrl);
    await new Promise((resolve, reject) => {
      this.ws.addEventListener('open', resolve, { once: true });
      this.ws.addEventListener('error', reject, { once: true });
    });
    this.ws.addEventListener('message', (message) => {
      const data = JSON.parse(message.data);
      if (data.id && this.pending.has(data.id)) {
        const { resolve, reject } = this.pending.get(data.id);
        this.pending.delete(data.id);
        if (data.error) reject(new Error(`${data.error.message} (${data.error.code})`));
        else resolve(data.result);
      } else if (data.method) {
        this.events.push({ at: Date.now(), method: data.method, sessionId: data.sessionId, params: data.params });
      }
    });
  }

  send(method, params = {}, sessionId) {
    const id = this.nextId++;
    this.calls.push(method);
    const message = { id, method, params };
    if (sessionId) message.sessionId = sessionId;
    return new Promise((resolve, reject) => {
      this.pending.set(id, { resolve, reject });
      this.ws.send(JSON.stringify(message));
      setTimeout(() => {
        if (this.pending.has(id)) {
          this.pending.delete(id);
          reject(new Error(`${method} timed out`));
        }
      }, 10000);
    });
  }

  async tryCall(method, params, sessionId) {
    try {
      return { ok: true, result: await this.send(method, params, sessionId) };
    } catch (error) {
      return { ok: false, error: error.message };
    }
  }

  drainEvents() {
    const events = this.events;
    this.events = [];
    return events;
  }

  close() {
    this.ws.close();
  }
}

async function fetchJson(url) {
  try {
    const response = await fetch(url);
    return { ok: response.ok, status: response.status, body: await response.json() };
  } catch (error) {
    return { ok: false, error: error.message };
  }
}

function histogramCounts(histograms) {
  const counts = {};
  for (const histogram of histograms ?? []) {
    counts[histogram.name] = { count: histogram.count, sum: histogram.sum };
  }
  return counts;
}

// One read-only snapshot as seen by the observer. Attaching a session to a
// page target is required for page-level queries; it does not touch any
// recorder, but it does flip that target's `attached` flag for other
// observers, which is why `attached` is also read before attaching.
async function snapshot(observer, httpBase, label) {
  observer.drainEvents();
  const started = Date.now();
  const targets = await observer.tryCall('Target.getTargets', { filter: [{}] });
  const contexts = await observer.tryCall('Target.getBrowserContexts');
  const version = await observer.tryCall('Browser.getVersion');
  const commandLine = await observer.tryCall('Browser.getBrowserCommandLine');
  const histograms = await observer.tryCall('Browser.getHistograms', { query: '' });
  const categories = await observer.tryCall('Tracing.getCategories');
  const jsonList = await fetchJson(`${httpBase}/json/list`);
  const jsonVersion = await fetchJson(`${httpBase}/json/version`);

  const pages = [];
  for (const info of targets.ok ? targets.result.targetInfos : []) {
    if (info.type !== 'page') continue;
    const page = { targetId: info.targetId, url: info.url, attachedBeforeObserver: info.attached };
    page.devToolsTarget = await observer.tryCall('Target.getDevToolsTarget', { targetId: info.targetId });
    page.window = await observer.tryCall('Browser.getWindowForTarget', { targetId: info.targetId });
    const attach = await observer.tryCall('Target.attachToTarget', { targetId: info.targetId, flatten: true });
    if (attach.ok) {
      const sessionId = attach.result.sessionId;
      page.targetInfo = await observer.tryCall('Target.getTargetInfo', { targetId: info.targetId }, sessionId);
      const frameTree = await observer.tryCall('Page.getFrameTree', {}, sessionId);
      page.frameCount = frameTree.ok ? countFrames(frameTree.result.frameTree) : frameTree.error;
      page.layoutMetrics = await observer.tryCall('Page.getLayoutMetrics', {}, sessionId);
      if (page.layoutMetrics.ok) page.layoutMetrics = { cssVisualViewport: page.layoutMetrics.result.cssVisualViewport };
      page.documentState = await observer.tryCall(
        'Runtime.evaluate',
        {
          expression:
            '({visibility: document.visibilityState, focus: document.hasFocus(), hidden: document.hidden, mediaSession: typeof navigator.mediaSession, displayCapture: !!(navigator.mediaDevices && navigator.mediaDevices.getDisplayMedia)})',
          returnByValue: true,
        },
        sessionId,
      );
      if (page.documentState.ok) page.documentState = page.documentState.result.result.value;
      await observer.tryCall('Target.detachFromTarget', { sessionId });
    } else {
      page.attachError = attach.error;
    }
    pages.push(page);
  }

  const events = observer.drainEvents();
  return {
    label,
    at: started,
    durationMs: Date.now() - started,
    targets: targets.ok
      ? targets.result.targetInfos.map((t) => ({ targetId: t.targetId, type: t.type, url: t.url, title: t.title, attached: t.attached, browserContextId: t.browserContextId, subtype: t.subtype }))
      : targets.error,
    contexts: contexts.ok ? contexts.result.browserContextIds : contexts.error,
    version: version.ok ? version.result : version.error,
    commandLine: commandLine.ok ? commandLine.result.arguments : commandLine.error,
    histograms: histograms.ok ? histogramCounts(histograms.result.histograms) : histograms.error,
    tracingCategoryCount: categories.ok ? categories.result.categories.length : categories.error,
    jsonList: jsonList.ok
      ? jsonList.body.map((entry) => ({ id: entry.id, type: entry.type, url: entry.url, keys: Object.keys(entry).sort(), hasWebSocketUrl: 'webSocketDebuggerUrl' in entry }))
      : jsonList,
    jsonVersion: jsonVersion.ok ? jsonVersion.body : jsonVersion,
    pages,
    observerEvents: events.map((event) => event.method),
  };
}

function countFrames(frame) {
  return 1 + (frame.childFrames ?? []).reduce((total, child) => total + countFrames(child), 0);
}

// Structured difference between two snapshots, restricted to signals that
// could plausibly indicate recorder state.
function diffSnapshots(before, after) {
  const diff = {};
  const beforeTargets = new Map((Array.isArray(before.targets) ? before.targets : []).map((t) => [t.targetId, t]));
  const afterTargets = new Map((Array.isArray(after.targets) ? after.targets : []).map((t) => [t.targetId, t]));
  diff.targetsAdded = [...afterTargets.values()].filter((t) => !beforeTargets.has(t.targetId)).map((t) => ({ type: t.type, url: t.url, attached: t.attached }));
  diff.targetsRemoved = [...beforeTargets.values()].filter((t) => !afterTargets.has(t.targetId)).map((t) => ({ type: t.type, url: t.url }));
  diff.targetsChanged = [];
  for (const [id, t] of afterTargets) {
    const previous = beforeTargets.get(id);
    if (!previous) continue;
    const changes = {};
    for (const key of ['attached', 'url', 'type', 'title']) {
      if (previous[key] !== t[key]) changes[key] = { before: previous[key], after: t[key] };
    }
    if (Object.keys(changes).length) diff.targetsChanged.push({ type: t.type, url: t.url, changes });
  }
  diff.contextsAdded = (Array.isArray(after.contexts) ? after.contexts : []).filter((c) => !(before.contexts ?? []).includes(c)).length;
  diff.histogramsChanged = [];
  if (typeof before.histograms === 'object' && typeof after.histograms === 'object') {
    const names = new Set([...Object.keys(before.histograms), ...Object.keys(after.histograms)]);
    for (const name of names) {
      const b = before.histograms[name] ?? { count: 0, sum: 0 };
      const a = after.histograms[name] ?? { count: 0, sum: 0 };
      if (b.count !== a.count) diff.histogramsChanged.push({ name, countBefore: b.count, countAfter: a.count, relevant: HISTOGRAM_PATTERN.test(name) });
    }
  }
  diff.relevantHistogramsChanged = diff.histogramsChanged.filter((h) => h.relevant);
  diff.tracingCategoryCount = { before: before.tracingCategoryCount, after: after.tracingCategoryCount };
  diff.jsonListChanged = JSON.stringify((before.jsonList ?? []).map?.((e) => [e.type, e.keys, e.hasWebSocketUrl])) !== JSON.stringify((after.jsonList ?? []).map?.((e) => [e.type, e.keys, e.hasWebSocketUrl]));
  diff.pagesChanged = [];
  const beforePages = new Map(before.pages.map((p) => [p.targetId, p]));
  for (const page of after.pages) {
    const previous = beforePages.get(page.targetId);
    if (!previous) {
      diff.pagesChanged.push({ url: page.url, added: true, devToolsTarget: page.devToolsTarget });
      continue;
    }
    const changes = {};
    for (const key of ['attachedBeforeObserver', 'frameCount', 'documentState', 'devToolsTarget', 'attachError']) {
      if (JSON.stringify(previous[key]) !== JSON.stringify(page[key])) changes[key] = { before: previous[key], after: page[key] };
    }
    if (Object.keys(changes).length) diff.pagesChanged.push({ url: page.url, changes });
  }
  diff.observerEvents = after.observerEvents;
  return diff;
}

// ------------------------------------------------------------------- main

async function main() {
  await mkdir(OUTPUT_DIR, { recursive: true });
  const observations = { startedAt: new Date().toISOString(), browserPath: BROWSER, recorders: {} };

  // ---- run 1: recorders started by a host client on a plain launch
  const browser = await launchBrowser();
  let host;
  let observer;
  try {
    host = await chromium.connectOverCDP(browser.httpBase);
    observations.playwrightCoreVersion = (await import('playwright-core/package.json', { with: { type: 'json' } })).default.version;
    const context = host.contexts()[0];
    const page = await context.newPage();
    await page.setContent('<!doctype html><title>probe</title><form><input id="p" type="password" value="x"></form>');

    observer = new Observer(browser.wsUrl);
    await observer.connect();
    await observer.send('Target.setDiscoverTargets', { discover: true });
    observations.versions = {
      browser: (await observer.send('Browser.getVersion')),
      httpVersion: (await fetchJson(`${browser.httpBase}/json/version`)).body,
    };

    const baseline = await snapshot(observer, browser.httpBase, 'baseline');
    observations.baseline = baseline;
    observations.readOnlyCallsUsed = [...new Set(observer.calls)].sort();

    const hostBrowserSession = await host.newBrowserCDPSession();
    const hostPageSession = await context.newCDPSession(page);

    const recorders = [
      {
        name: 'chromium-tracing',
        description: 'Tracing.start by another session (browser-level)',
        start: () => hostBrowserSession.send('Tracing.start', { categories: 'devtools.timeline,disabled-by-default-devtools.screenshot', transferMode: 'ReportEvents' }),
        stop: async () => {
          const done = new Promise((resolve) => hostBrowserSession.once('Tracing.tracingComplete', resolve));
          await hostBrowserSession.send('Tracing.end');
          await Promise.race([done, sleep(5000)]);
        },
      },
      {
        name: 'page-screencast',
        description: 'Page.startScreencast by another session on the page',
        start: async () => {
          hostPageSession.on('Page.screencastFrame', (frame) => {
            hostPageSession.send('Page.screencastFrameAck', { sessionId: frame.sessionId }).catch(() => {});
          });
          await hostPageSession.send('Page.startScreencast', { format: 'jpeg', quality: 20, everyNthFrame: 1 });
          await page.evaluate(() => { document.body.append('frame trigger'); });
        },
        stop: () => hostPageSession.send('Page.stopScreencast'),
      },
      {
        name: 'page-screen-recording',
        description: 'Page.startScreenRecording (experimental video recording) by another session',
        start: () => hostPageSession.send('Page.startScreenRecording', { frameRate: 5 }),
        stop: () => hostPageSession.send('Page.stopScreenRecording'),
      },
      {
        name: 'page-screenshot',
        description: 'Page.captureScreenshot by another session (instantaneous; observed before/after only)',
        start: async () => {
          for (let i = 0; i < 3; i += 1) await page.screenshot({ type: 'png' });
        },
        stop: async () => {},
      },
      {
        name: 'playwright-tracing',
        description: 'context.tracing.start({screenshots, snapshots}) on the connectOverCDP default context',
        start: () => context.tracing.start({ screenshots: true, snapshots: true }),
        stop: () => context.tracing.stop({ path: join(OUTPUT_DIR, 'playwright-trace.zip') }),
      },
      {
        name: 'playwright-har',
        description: 'browser.newContext({recordHar}) on connectOverCDP',
        start: async function () {
          this.context = await host.newContext({ recordHar: { path: join(OUTPUT_DIR, 'record.har') } });
          this.page = await this.context.newPage();
          await this.page.setContent('<input id="q">');
        },
        stop: async function () {
          if (this.context) await this.context.close();
        },
      },
      {
        name: 'playwright-video',
        description: 'browser.newContext({recordVideo}) on connectOverCDP',
        start: async function () {
          this.context = await host.newContext({ recordVideo: { dir: join(OUTPUT_DIR, 'video') } });
          this.page = await this.context.newPage();
          await this.page.setContent('<input id="q">');
        },
        stop: async function () {
          if (this.context) await this.context.close();
        },
      },
      {
        name: 'devtools-open-via-protocol',
        description: 'Target.openDevTools issued by the host for the page',
        start: async function () {
          const targetId = (await hostPageSession.send('Target.getTargetInfo')).targetInfo.targetId;
          this.targetId = targetId;
          await hostBrowserSession.send('Target.openDevTools', { targetId });
          await sleep(1500);
        },
        stop: async function () {
          // Close the DevTools page target if we can find it; host-side mutation.
          const devtools = await hostBrowserSession.send('Target.getDevToolsTarget', { targetId: this.targetId }).catch(() => ({}));
          if (devtools.targetId) await hostBrowserSession.send('Target.closeTarget', { targetId: devtools.targetId }).catch(() => {});
          await sleep(500);
        },
      },
    ];

    let previousAfter = baseline;
    for (const recorder of recorders) {
      const record = { description: recorder.description };
      observations.recorders[recorder.name] = record;
      try {
        await recorder.start();
        record.started = true;
        await sleep(700);
        const during = await snapshot(observer, browser.httpBase, `${recorder.name}:during`);
        record.duringDiff = diffSnapshots(baseline, during);
        // Incremental view: what changed since the previous recorder stopped.
        record.incrementalDiff = diffSnapshots(previousAfter, during);
        record.duringSnapshot = during;
      } catch (error) {
        record.started = false;
        record.startError = String(error?.message ?? error).split('\n')[0];
      }
      try {
        await recorder.stop();
        record.stopped = true;
      } catch (error) {
        record.stopped = false;
        record.stopError = String(error?.message ?? error).split('\n')[0];
      }
      await sleep(500);
      const after = await snapshot(observer, browser.httpBase, `${recorder.name}:after`);
      record.afterDiff = diffSnapshots(baseline, after);
      record.stopDiff = diffSnapshots(record.duringSnapshot ?? previousAfter, after);
      previousAfter = after;
    }
    observations.readOnlyCallsUsed = [...new Set(observer.calls)].sort();
    await hostBrowserSession.detach().catch(() => {});
    await hostPageSession.detach().catch(() => {});
  } finally {
    observer?.close();
    await host?.close().catch(() => {});
    await browser.close();
  }

  // ---- run 2: DevTools auto-opened by the browser itself
  const browser2 = await launchBrowser(['--auto-open-devtools-for-tabs']);
  let observer2;
  try {
    observer2 = new Observer(browser2.wsUrl);
    await observer2.connect();
    await sleep(2500);
    const snap = await snapshot(observer2, browser2.httpBase, 'auto-open-devtools');
    observations.recorders['devtools-auto-open'] = {
      description: 'Browser launched with --auto-open-devtools-for-tabs; observer inspects the tab',
      started: true,
      snapshot: {
        targets: snap.targets,
        pages: snap.pages.map((p) => ({ url: p.url, devToolsTarget: p.devToolsTarget, attachedBeforeObserver: p.attachedBeforeObserver })),
        jsonList: snap.jsonList,
      },
    };
  } finally {
    observer2?.close();
    await browser2.close();
  }

  observations.finishedAt = new Date().toISOString();
  const outputPath = join(OUTPUT_DIR, 'observations.json');
  await writeFile(outputPath, JSON.stringify(observations, null, 2));
  printSummary(observations, outputPath);
}

function printSummary(observations, outputPath) {
  console.log(`browser: ${observations.versions?.browser?.product} protocol ${observations.versions?.browser?.protocolVersion}; playwright-core ${observations.playwrightCoreVersion}`);
  for (const [name, record] of Object.entries(observations.recorders)) {
    const d = record.incrementalDiff ?? record.duringDiff;
    const signal = d
      ? [
          d.targetsAdded.length ? `targets+${d.targetsAdded.map((t) => t.type + ':' + t.url.slice(0, 40)).join(',')}` : '',
          d.targetsChanged.length ? `targetsChanged=${JSON.stringify(d.targetsChanged)}` : '',
          d.relevantHistogramsChanged.length ? `histograms=${d.relevantHistogramsChanged.map((h) => h.name).join(',')}` : '',
          d.pagesChanged.length ? `pages=${JSON.stringify(d.pagesChanged)}` : '',
          d.jsonListChanged ? 'jsonListChanged' : '',
          d.tracingCategoryCount.before !== d.tracingCategoryCount.after ? 'categoriesChanged' : '',
        ].filter(Boolean).join(' | ')
      : record.snapshot
        ? `pages=${JSON.stringify(record.snapshot.pages)}`
        : `start failed: ${record.startError}`;
    console.log(`- ${name}: started=${record.started} ${record.startError ? 'error=' + record.startError : ''} ${signal || 'NO SIGNAL'}`);
  }
  console.log(`raw observations: ${outputPath}`);
}

await main();
