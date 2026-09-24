// Parity harness for the CDP filling clients.
//
// Launches a Chromium-family browser with a temporary profile and remote
// debugging, starts the fixture server, and for every scenario opens the
// listed pages from a separate Playwright host session, runs each client,
// and compares outcome, receipts, events, host liveness and target counts
// against the catalog and between the two implementations.
//
// Usage: node harness/main.mjs --output-dir DIR --native-bin PATH
//        [--browser PATH] [--headed] [--implementation native|reference|both]
//        [--scenario ID]...
import { spawn } from 'node:child_process';
import { mkdir, mkdtemp, readFile, rm, writeFile } from 'node:fs/promises';
import { createRequire } from 'node:module';
import { tmpdir } from 'node:os';
import { dirname, join, resolve } from 'node:path';
import { setTimeout as sleep } from 'node:timers/promises';
import { fileURLToPath } from 'node:url';

const here = dirname(fileURLToPath(import.meta.url));
const labRoot = resolve(here, '..');
const require = createRequire(join(labRoot, 'reference', 'package.json'));
const { chromium } = require('playwright-core');
const playwrightVersion = require('playwright-core/package.json').version;

const options = parseArgs(process.argv.slice(2));
const catalog = JSON.parse(await readFile(join(labRoot, 'scenarios.json'), 'utf8'));
const implementations = options.implementations;
// Parity is measured against the reference client when it runs; otherwise
// against the first listed implementation.
const parityBaseline = implementations.includes('reference') ? 'reference' : implementations[0];

await mkdir(options.outputDir, { recursive: true });

// Fixture server.
const fixture = spawn(process.execPath, [join(labRoot, 'fixtures', 'server.mjs')], {
  stdio: ['pipe', 'pipe', 'inherit'],
});
const origins = await new Promise((resolveOrigins, reject) => {
  fixture.stdout.once('data', (data) => resolveOrigins(JSON.parse(data.toString())));
  fixture.once('exit', (code) => reject(new Error(`fixture server exited with ${code}`)));
});

// Browser.
const profile = await mkdtemp(join(tmpdir(), 'cdp-lab-'));
const browserArgs = [
  '--remote-debugging-port=0',
  `--user-data-dir=${profile}`,
  '--no-first-run',
  '--no-default-browser-check',
  ...(options.headed ? [] : ['--headless']),
  'about:blank',
];
const browserProcess = spawn(options.browser, browserArgs, { stdio: 'ignore' });
let port;
for (let attempt = 0; attempt < 200 && !port; attempt += 1) {
  try {
    port = (await readFile(join(profile, 'DevToolsActivePort'), 'utf8')).split('\n')[0].trim();
  } catch {
    await sleep(100);
  }
}
if (!port) {
  await shutdown();
  throw new Error('the browser did not publish DevToolsActivePort');
}
const endpoint = `http://127.0.0.1:${port}`;

const host = await chromium.connectOverCDP(endpoint);
const browserCdp = await host.newBrowserCDPSession();
const browserVersion = await browserCdp.send('Browser.getVersion');
const commandLine = await browserCdp.send('Browser.getBrowserCommandLine').catch(() => ({ arguments: [] }));
const versionDocument = await (await fetch(`${endpoint}/json/version`)).json();

const substitute = (text) => text.replaceAll('{A}', origins.originA).replaceAll('{B}', origins.originB);
const originOf = (key) => (key === 'A' ? origins.originA : origins.originB);

const evidence = {
  generatedAt: new Date().toISOString(),
  versions: {
    browser: browserVersion.product,
    browserUserAgent: browserVersion.userAgent,
    browserRevision: browserVersion.revision,
    browserFromJsonVersion: versionDocument.Browser,
    protocol: browserVersion.protocolVersion,
    jsVersion: browserVersion.jsVersion,
    playwrightCore: playwrightVersion,
    node: process.version,
    platform: `${process.platform} ${process.arch}`,
    headed: options.headed,
  },
  browserCommandLine: commandLine.arguments,
  browserPath: options.browser,
  fixtureOrigins: origins,
  implementations,
  scenarios: [],
};

async function pageTargets() {
  const { targetInfos } = await browserCdp.send('Target.getTargets');
  return targetInfos.filter((info) => info.type === 'page').map((info) => info.targetId).sort();
}

async function iframeTargets() {
  const { targetInfos } = await browserCdp.send('Target.getTargets');
  return targetInfos.filter((info) => info.type === 'iframe').map((info) => info.url);
}

async function openScenarioPages(scenario) {
  const contexts = { 0: host.contexts()[0] };
  const opened = [];
  for (const open of scenario.open) {
    if (!contexts[open.context]) contexts[open.context] = await host.newContext();
    const page = await contexts[open.context].newPage();
    await page.goto(`${originOf(open.origin)}/pages/${open.page}${open.query ?? ''}`, { waitUntil: 'load' });
    const instances = {};
    for (const frame of page.frames()) {
      try {
        const info = await frame.evaluate(() => ({
          instance: window.__fixture?.instance,
          frame: document.body.dataset.frame || 'main',
        }));
        if (info.instance) instances[info.instance] = { ref: open.ref, frame: info.frame, url: frame.url() };
      } catch {
        // A frame without the fixture script (about:blank) is not attributable.
      }
    }
    opened.push({ ...open, page, instances });
  }
  return { contexts, opened };
}

async function pageTargetId(page) {
  const { targetInfos } = await browserCdp.send('Target.getTargets');
  return targetInfos.find((info) => info.type === 'page' && info.url === page.url())?.targetId ?? null;
}

// Scenario-specific host-side preparation after the pages are open.
async function hostSetup(scenario, pages) {
  const main = pages.opened[0];
  if (scenario.hostSetup === 'await-workers') {
    await main.page.evaluate(() => Promise.all([window.__workerReady, window.__serviceWorkerReady]));
    await sleep(200);
  } else if (scenario.hostSetup === 'open-devtools') {
    const targetId = await pageTargetId(main.page);
    if (!targetId) throw new Error('cannot find the page target for DevTools');
    await browserCdp.send('Target.openDevTools', { targetId });
    // Wait until the browser reports the DevTools target for the page and
    // lists it as a page target, so the target set is stable before the run.
    const started = performance.now();
    while (performance.now() - started < 5000) {
      const devtools = await browserCdp.send('Target.getDevToolsTarget', { targetId }).catch(() => ({}));
      if (devtools.targetId) {
        const { targetInfos } = await browserCdp.send('Target.getTargets');
        if (targetInfos.some((info) => info.type === 'page' && info.targetId === devtools.targetId)) {
          await sleep(300);
          return;
        }
      }
      await sleep(100);
    }
    throw new Error('DevTools did not open for the page');
  }
}

async function hostTeardown(scenario, pages) {
  if (scenario.hostSetup === 'open-devtools') {
    const targetId = await pageTargetId(pages.opened[0].page);
    const devtools = targetId ? await browserCdp.send('Target.getDevToolsTarget', { targetId }).catch(() => ({})) : {};
    if (devtools.targetId) await browserCdp.send('Target.closeTarget', { targetId: devtools.targetId }).catch(() => {});
  }
}

async function closeScenarioPages({ contexts, opened }) {
  for (const entry of opened) await entry.page.close().catch(() => {});
  for (const [index, context] of Object.entries(contexts)) {
    if (index !== '0') await context.close().catch(() => {});
  }
}

function clientCommand(implementation, request, value) {
  const args = ['--endpoint', endpoint, '--page-url', substitute(request.pageUrl), '--selector', request.selector];
  if (request.pageMatch) args.push('--page-match', request.pageMatch);
  if (request.contextIndex != null) args.push('--context-index', String(request.contextIndex));
  for (const frameSelector of request.frameSelectors ?? []) args.push('--frame-selector', frameSelector);
  if (request.timeoutMs) args.push('--timeout-ms', String(request.timeoutMs));
  if (implementation === 'product') {
    // The product reads the value from a configured env credential, never
    // from stdin or argv. The delay hook is a debug-build environment
    // variable so the public CLI carries no test option.
    const env = {
      ...process.env,
      AGENTENV_FILE: options.productConfig,
      AGENTENV_NO_PROJECT: '1',
      AGENTENV_LAB_VALUE: value,
    };
    if (request.delayBeforeInsertMs) env.AGENTENV_FILL_TEST_DELAY_MS = String(request.delayBeforeInsertMs);
    return {
      command: options.productBin,
      args: ['--json', 'credential', 'fill', 'lab_value', '--backend', 'cdp', ...args],
      env,
      valueOnStdin: false,
    };
  }
  if (request.delayBeforeInsertMs) args.push('--delay-before-insert-ms', String(request.delayBeforeInsertMs));
  if (implementation === 'native') return { command: options.nativeBin, args, env: process.env, valueOnStdin: true };
  return { command: process.execPath, args: [join(labRoot, 'reference', 'fill.mjs'), ...args], env: process.env, valueOnStdin: true };
}

// Maps the product's CLI contract (success JSON of version/backend/effect on
// stdout; `credential-fill: <reason>: <message>` on stderr with exit 8 or 11)
// onto the lab's outcome schema.
function productResult(exitCode, stdout, stderr) {
  if (exitCode === 0) {
    try {
      const document = JSON.parse(stdout.trim());
      return { outcome: 'filled', reason: null, message: '', details: { effect: document.effect, backend: document.backend }, versions: null };
    } catch {
      return { outcome: 'error', reason: 'unparseable-output', message: stdout.slice(0, 200) };
    }
  }
  const line = stderr.split('\n').find((text) => text.startsWith('credential-fill: ')) ?? '';
  const match = /^credential-fill: ([a-z-]+): (.*)$/.exec(line);
  return {
    outcome: 'error',
    reason: match ? match[1] : `exit-${exitCode}`,
    message: match ? match[2] : stderr.slice(0, 200),
    details: { exitCode, mutationUncertain: exitCode === 11 },
    versions: null,
  };
}

async function runClient(implementation, scenario) {
  const value = catalog.values[scenario.value];
  const { command, args, env, valueOnStdin } = clientCommand(implementation, scenario.request, value);
  const started = performance.now();
  const child = spawn(command, args, { stdio: ['pipe', 'pipe', 'pipe'], env });
  child.stdin.end(valueOnStdin ? value : '');
  let stdout = '';
  let stderr = '';
  child.stdout.on('data', (data) => { stdout += data; });
  child.stderr.on('data', (data) => { stderr += data; });
  const exitCode = await new Promise((resolveExit) => child.on('close', resolveExit));
  const elapsedMs = Math.round(performance.now() - started);
  let result;
  if (implementation === 'product') {
    result = productResult(exitCode, stdout, stderr);
  } else {
    try {
      result = JSON.parse(stdout.trim());
    } catch {
      result = { outcome: 'error', reason: 'unparseable-output', message: stdout.slice(0, 200) };
    }
  }
  return { exitCode, elapsedMs, result, stderr: stderr.trim() };
}

async function evaluateRun(scenario, implementation) {
  await fetch(`${origins.originA}/receipt`, { method: 'DELETE' });
  const pages = await openScenarioPages(scenario);
  await hostSetup(scenario, pages);
  const targetsBefore = await pageTargets();
  const iframesDuring = await iframeTargets();

  const run = await runClient(implementation, scenario);

  await sleep(350);
  const { receipts } = await (await fetch(`${origins.originA}/receipt`)).json();
  const instances = Object.assign({}, ...pages.opened.map((entry) => entry.instances));
  const attributed = receipts
    .filter((receipt) => instances[receipt.instance])
    .map((receipt) => ({
      ...instances[receipt.instance],
      field: receipt.field,
      value: receipt.value,
      valueSource: receipt.valueSource,
      origin: receipt.origin,
      events: receipt.events,
    }));
  const stale = receipts.filter((receipt) => !instances[receipt.instance]).map((receipt) => receipt.field);
  const targetsAfter = await pageTargets();

  // Host liveness: the host session must still drive every page it opened.
  const host = { alive: true, snapshots: [] };
  for (const entry of pages.opened) {
    try {
      if ((await entry.page.evaluate('1 + 1')) !== 2) host.alive = false;
      const snapshot = await entry.page.evaluate(() => window.__fixture?.snapshot() ?? null);
      host.snapshots.push({ ref: entry.ref, href: snapshot?.href ?? entry.page.url(), fields: snapshot?.fields ?? null });
    } catch (error) {
      host.alive = false;
      host.snapshots.push({ ref: entry.ref, error: String(error.message ?? error).split('\n')[0] });
    }
  }

  const expect = scenario.expect;
  const problems = [];
  if (run.result.outcome !== expect.outcome) problems.push(`outcome ${run.result.outcome} != ${expect.outcome}`);
  if (expect.outcome === 'error' && run.result.reason !== expect.reason) problems.push(`reason ${run.result.reason} != ${expect.reason}`);
  if (expect.outcome === 'filled' && run.exitCode !== 0) problems.push(`exit ${run.exitCode} != 0`);
  if (expect.outcome === 'error' && run.exitCode !== 8) problems.push(`exit ${run.exitCode} != 8`);
  if (expect.receipt) {
    const want = expect.receipt;
    const hit = attributed.find(
      (receipt) =>
        receipt.ref === want.ref &&
        receipt.frame === want.frame &&
        receipt.field === want.field &&
        receipt.value === catalog.values[want.value] &&
        (!want.valueSource || receipt.valueSource === want.valueSource) &&
        (!want.frameOrigin || receipt.origin === originOf(want.frameOrigin)),
    );
    if (!hit) problems.push('expected receipt missing or wrong');
    else if (JSON.stringify(hit.events) !== JSON.stringify(catalog.filledEvents)) problems.push(`events ${hit.events.join(',')} != ${catalog.filledEvents.join(',')}`);
    const extra = attributed.filter((receipt) => receipt !== hit);
    if (extra.length > 0) problems.push(`unexpected receipts: ${extra.map((r) => `${r.ref}/${r.frame}/${r.field}`).join(', ')}`);
    // Independent DOM-level confirmation from the host session. The fixture
    // snapshot lists light-DOM fields of the main document only, so fields
    // inside shadow roots or child frames are confirmed by receipt alone.
    const snapshot = host.snapshots.find((s) => s.ref === want.ref);
    if (want.frame === 'main' && snapshot?.fields && want.field in snapshot.fields && snapshot.fields[want.field] !== catalog.values[want.value]) {
      problems.push('host snapshot of the field differs from the value');
    }
  } else if (attributed.length > 0) {
    problems.push(`mutation observed: ${attributed.map((r) => `${r.ref}/${r.frame}/${r.field}`).join(', ')}`);
  }
  if (!host.alive) problems.push('host session lost a page');
  if (JSON.stringify(targetsBefore) !== JSON.stringify(targetsAfter)) problems.push('page targets changed');
  await hostTeardown(scenario, pages);
  if (expect.oopif) {
    const frameUrl = attributed[0]?.url;
    if (!iframesDuring.some((url) => url === frameUrl)) problems.push('expected an out-of-process iframe target for the filled frame');
  }
  if (typeof run.result.message === 'string' && run.result.message.includes(catalog.values[scenario.value])) problems.push('value leaked into the message');
  if (run.stderr.includes(catalog.values[scenario.value])) problems.push('value leaked into stderr');
  if (implementation === 'product') {
    // The product distinguishes "nothing changed" (8) from "may have changed"
    // (11); the catalog's error scenarios all fail before insertion.
    if (expect.outcome === 'error' && run.exitCode !== 8) problems.push(`product exit ${run.exitCode} != 8`);
    if (expect.outcome === 'filled' && run.result.details?.effect !== 'field-filled') problems.push('product effect is not field-filled');
  }

  await closeScenarioPages(pages);
  return {
    pass: problems.length === 0,
    problems,
    exitCode: run.exitCode,
    elapsedMs: run.elapsedMs,
    outcome: run.result.outcome,
    reason: run.result.reason ?? null,
    message: run.result.message ?? '',
    details: run.result.details ?? null,
    versions: run.result.versions ?? null,
    receipts: attributed.map((r) => ({ ref: r.ref, frame: r.frame, field: r.field, valueSource: r.valueSource, origin: r.origin, valueMatches: r.value === catalog.values[scenario.value], events: r.events })),
    staleReceipts: stale,
    hostAlive: host.alive,
    hostSnapshots: host.snapshots.map((s) => ({ ref: s.ref, href: s.href, error: s.error })),
    pageTargets: { before: targetsBefore.length, after: targetsAfter.length },
    iframeTargetsDuring: iframesDuring,
    stderr: run.stderr,
  };
}

function compareRuns(subjectName, subject, baselineName, baseline) {
  const differences = [];
  const compare = (label, a, b) => {
    if (JSON.stringify(a) !== JSON.stringify(b)) differences.push(`${label}: ${subjectName}=${JSON.stringify(a)} ${baselineName}=${JSON.stringify(b)}`);
  };
  compare('outcome', subject.outcome, baseline.outcome);
  compare('reason', subject.reason, baseline.reason);
  compare('exitCode', subject.exitCode, baseline.exitCode);
  const strip = (receipts) => receipts.map((r) => ({ ref: r.ref, frame: r.frame, field: r.field, valueSource: r.valueSource, valueMatches: r.valueMatches, events: r.events }));
  compare('receipts', strip(subject.receipts), strip(baseline.receipts));
  compare('hostAlive', subject.hostAlive, baseline.hostAlive);
  compare('pageTargetsStable', subject.pageTargets.before === subject.pageTargets.after, baseline.pageTargets.before === baseline.pageTargets.after);
  return { equal: differences.length === 0, differences };
}

async function shutdown() {
  await host?.close().catch(() => {});
  browserProcess.kill('SIGKILL');
  fixture.stdin.end();
  await sleep(300);
  await rm(profile, { recursive: true, force: true });
}

try {
  for (const scenario of catalog.scenarios) {
    if (options.scenarios.size > 0 && !options.scenarios.has(scenario.id)) continue;
    const record = { id: scenario.id, expect: scenario.expect, notes: scenario.notes ?? null };
    // A scenario may apply to a subset of implementations (for example a
    // product-only preflight rule); the others are skipped and parity is
    // compared only among those that ran.
    const applicable = implementations.filter((implementation) => !scenario.implementations || scenario.implementations.includes(implementation));
    if (applicable.length === 0) {
      console.log(`SKIP ${scenario.id} (applies to ${scenario.implementations.join(', ')})`);
      continue;
    }
    for (const implementation of implementations) {
      if (!applicable.includes(implementation)) {
        record[implementation] = { pass: true, skipped: true, problems: [], exitCode: null, elapsedMs: 0, outcome: 'skipped', reason: null, message: '', details: null, versions: null, receipts: [], staleReceipts: [], hostAlive: true, hostSnapshots: [], pageTargets: { before: 0, after: 0 }, iframeTargetsDuring: [], stderr: '' };
        continue;
      }
      record[implementation] = await evaluateRun(scenario, implementation);
      const run = record[implementation];
      console.log(`${run.pass ? 'PASS' : 'FAIL'} ${implementation.padEnd(9)} ${scenario.id} -> ${run.outcome}${run.reason ? '/' + run.reason : ''} exit=${run.exitCode} ${run.elapsedMs}ms${run.pass ? '' : ' :: ' + run.problems.join('; ')}`);
    }
    if (implementations.length >= 2 && applicable.length >= 2) {
      const baseline = applicable.includes(parityBaseline) ? parityBaseline : applicable[0];
      const differences = [];
      for (const implementation of applicable) {
        if (implementation === baseline) continue;
        differences.push(...compareRuns(implementation, record[implementation], baseline, record[baseline]).differences);
      }
      record.parity = { baseline, equal: differences.length === 0, differences };
      if (!record.parity.equal) console.log(`DIFF ${scenario.id} :: ${record.parity.differences.join('; ')}`);
    }
    record.pass = implementations.every((implementation) => record[implementation].pass) && (record.parity?.equal ?? true);
    evidence.scenarios.push(record);
  }
} finally {
  await shutdown();
}

const total = evidence.scenarios.length;
const passed = evidence.scenarios.filter((record) => record.pass).length;
const parityCompared = evidence.scenarios.filter((record) => record.parity).length;
const parityEqual = evidence.scenarios.filter((record) => record.parity?.equal).length;
evidence.summary = {
  total,
  passed,
  failed: total - passed,
  parityBaseline: implementations.length >= 2 ? parityBaseline : null,
  parityCompared: implementations.length >= 2 ? parityCompared : 0,
  parityEqual: implementations.length >= 2 ? parityEqual : null,
  nativeVersions: evidence.scenarios.find((record) => record.native)?.native.versions ?? null,
  referenceVersions: evidence.scenarios.find((record) => record.reference)?.reference.versions ?? null,
  productBin: options.productBin,
};
await writeFile(join(options.outputDir, 'evidence.json'), `${JSON.stringify(evidence, null, 2)}\n`);
await writeFile(join(options.outputDir, 'summary.md'), renderSummary(evidence));
console.log(`\n${passed}/${total} scenarios passed${implementations.length >= 2 ? `, ${parityEqual}/${parityCompared} parity-equal against ${parityBaseline}` : ''}; evidence in ${options.outputDir}`);
process.exit(passed === total ? 0 : 1);

function renderSummary(evidence) {
  const lines = [];
  lines.push('# CDP filling parity run', '');
  lines.push(`Generated ${evidence.generatedAt}.`, '');
  lines.push('| Component | Version |', '| --- | --- |');
  for (const [key, value] of Object.entries(evidence.versions)) lines.push(`| ${key} | ${value} |`);
  if (evidence.summary.nativeVersions) lines.push(`| native client | ${evidence.summary.nativeVersions.library} |`);
  lines.push('', `Browser: \`${evidence.browserPath}\``, '');
  lines.push(`Result: ${evidence.summary.passed}/${evidence.summary.total} scenarios passed` + (evidence.summary.parityEqual != null ? `; ${evidence.summary.parityEqual}/${evidence.summary.parityCompared} compared scenarios identical between the implementations (baseline ${evidence.summary.parityBaseline}).` : '.'), '');
  const header = ['Scenario', 'Expected', ...evidence.implementations.map((i) => `${i} outcome`), ...evidence.implementations.map((i) => `${i} ms`), 'Parity', 'Problems'];
  lines.push(`| ${header.join(' | ')} |`, `| ${header.map(() => '---').join(' | ')} |`);
  for (const record of evidence.scenarios) {
    const expected = record.expect.outcome === 'filled' ? 'filled' : `error/${record.expect.reason}`;
    const outcomes = evidence.implementations.map((i) => `${record[i].outcome}${record[i].reason ? '/' + record[i].reason : ''}${record[i].pass ? '' : ' ✗'}`);
    const timings = evidence.implementations.map((i) => String(record[i].elapsedMs));
    const parity = record.parity ? (record.parity.equal ? 'equal' : 'DIFF') : 'n/a';
    const problems = evidence.implementations.flatMap((i) => record[i].problems.map((p) => `${i}: ${p}`)).concat(record.parity?.differences ?? []).join('; ');
    lines.push(`| ${record.id} | ${expected} | ${outcomes.join(' | ')} | ${timings.join(' | ')} | ${parity} | ${problems} |`);
  }
  lines.push('');
  return `${lines.join('\n')}\n`;
}

function parseArgs(argv) {
  const parsed = { outputDir: null, nativeBin: null, productBin: null, productConfig: null, browser: null, headed: false, implementations: ['native', 'reference'], scenarios: new Set() };
  const aliases = { both: ['native', 'reference'], all: ['native', 'reference', 'product'] };
  for (let index = 0; index < argv.length; index += 1) {
    const flag = argv[index];
    const next = () => argv[++index];
    if (flag === '--output-dir') parsed.outputDir = resolve(next());
    else if (flag === '--native-bin') parsed.nativeBin = resolve(next());
    else if (flag === '--product-bin') parsed.productBin = resolve(next());
    else if (flag === '--product-config') parsed.productConfig = resolve(next());
    else if (flag === '--browser') parsed.browser = next();
    else if (flag === '--headed') parsed.headed = true;
    else if (flag === '--implementation') {
      const value = next();
      parsed.implementations = aliases[value] ?? value.split(',');
    } else if (flag === '--scenario') parsed.scenarios.add(next());
    else throw new Error(`unknown argument ${flag}`);
  }
  if (!parsed.outputDir || !parsed.browser) throw new Error('--output-dir and --browser are required');
  for (const implementation of parsed.implementations) {
    if (!['native', 'reference', 'product'].includes(implementation)) throw new Error(`unknown implementation ${implementation}`);
  }
  if (parsed.implementations.includes('native') && !parsed.nativeBin) throw new Error('--native-bin is required for the native implementation');
  if (parsed.implementations.includes('product') && !(parsed.productBin && parsed.productConfig)) throw new Error('--product-bin and --product-config are required for the product implementation');
  return parsed;
}
