// Reference filling client for the CDP parity lab, built on playwright-core.
//
// Implements the client contract in tests/cdp_lab/README.md using
// chromium.connectOverCDP(). It is the semantic baseline: page selection,
// frame chain, element state checks and the fill itself are Playwright's own
// definitions. The value never reaches stdout or stderr.

import { createRequire } from 'node:module';

import { chromium } from 'playwright-core';

const require = createRequire(import.meta.url);
const LIBRARY_VERSION = require('playwright-core/package.json').version;

const FILLABLE_INPUT_TYPES = new Set(['', 'text', 'password', 'email', 'search', 'tel', 'url', 'number']);

class FillError extends Error {
  constructor(reason, message, phase) {
    super(message);
    this.reason = reason;
    this.phase = phase;
  }
}

function usage(message) {
  process.stderr.write(`usage error: ${message}\n`);
  process.exit(1);
}

function parseArgs(argv) {
  const options = {
    endpoint: null,
    pageUrl: null,
    pageMatch: 'exact',
    contextIndex: null,
    frameSelectors: [],
    selector: null,
    timeoutMs: 10000,
    delayBeforeInsertMs: 0,
  };
  for (let index = 0; index < argv.length; index += 1) {
    const flag = argv[index];
    const next = () => {
      index += 1;
      if (index >= argv.length) usage(`${flag} requires a value`);
      return argv[index];
    };
    switch (flag) {
      case '--endpoint': options.endpoint = next(); break;
      case '--page-url': options.pageUrl = next(); break;
      case '--page-match': options.pageMatch = next(); break;
      case '--context-index': options.contextIndex = Number(next()); break;
      case '--frame-selector': options.frameSelectors.push(next()); break;
      case '--selector': options.selector = next(); break;
      case '--timeout-ms': options.timeoutMs = Number(next()); break;
      case '--delay-before-insert-ms': options.delayBeforeInsertMs = Number(next()); break;
      default: usage(`unknown argument ${flag}`);
    }
  }
  if (!options.endpoint) usage('--endpoint is required');
  if (!options.pageUrl) usage('--page-url is required');
  if (!options.selector) usage('--selector is required');
  if (!['exact', 'origin-path'].includes(options.pageMatch)) usage('--page-match must be exact or origin-path');
  if (!Number.isInteger(options.timeoutMs) || options.timeoutMs <= 0) usage('--timeout-ms must be a positive integer');
  if (!Number.isInteger(options.delayBeforeInsertMs) || options.delayBeforeInsertMs < 0) usage('--delay-before-insert-ms must be a non-negative integer');
  if (options.contextIndex !== null && (!Number.isInteger(options.contextIndex) || options.contextIndex < 0)) usage('--context-index must be a non-negative integer');
  return options;
}

class Deadline {
  constructor(totalMs) {
    this.end = performance.now() + totalMs;
  }

  remaining() {
    return Math.max(0, Math.ceil(this.end - performance.now()));
  }

  expired() {
    return performance.now() >= this.end;
  }

  /** Remaining budget for a Playwright call; throws timeout when exhausted. */
  budget(phase) {
    const left = this.remaining();
    if (left <= 0) throw new FillError('timeout', 'operation deadline expired', phase);
    return left;
  }
}

function pageMatches(url, wanted, mode) {
  if (mode === 'exact') return url === wanted;
  let actual;
  let expected;
  try {
    actual = new URL(url);
    expected = new URL(wanted);
  } catch {
    return false;
  }
  return actual.protocol === expected.protocol && actual.host === expected.host && actual.pathname === expected.pathname;
}

function isTimeoutError(error) {
  return error?.name === 'TimeoutError' || /Timeout \d+ms exceeded/.test(error?.message ?? '');
}

function isDetachedError(error) {
  const message = error?.message ?? '';
  return /not attached|detached|not connected|navigat|Execution context was destroyed|Target closed|Frame was detached|has been closed/i.test(message);
}

async function withSession(context, page, action) {
  const session = await context.newCDPSession(page);
  try {
    return await action(session);
  } finally {
    await session.detach().catch(() => {});
  }
}

/** Lists browser context ids in contract order: default context, then Target.getBrowserContexts order. */
async function contextOrder(browser, deadline) {
  const session = await browser.newBrowserCDPSession();
  try {
    const { browserContextIds } = await session.send('Target.getBrowserContexts');
    return { created: browserContextIds };
  } finally {
    await session.detach().catch(() => {});
  }
}

async function selectPage(browser, options, deadline, phase) {
  deadline.budget(phase);
  const { created } = await contextOrder(browser, deadline);
  if (options.contextIndex !== null && options.contextIndex > created.length) {
    throw new FillError('context-absent', `context index ${options.contextIndex} does not exist`, phase);
  }
  const matches = [];
  for (const context of browser.contexts()) {
    for (const page of context.pages()) {
      deadline.budget(phase);
      const url = page.url();
      if (!pageMatches(url, options.pageUrl, options.pageMatch)) continue;
      const contextId = await withSession(context, page, async (session) => {
        const { targetInfo } = await session.send('Target.getTargetInfo');
        return targetInfo.browserContextId ?? null;
      });
      const position = created.indexOf(contextId);
      const contextIndex = position === -1 ? 0 : position + 1;
      if (options.contextIndex !== null && contextIndex !== options.contextIndex) continue;
      matches.push({ page, url, contextIndex });
    }
  }
  if (matches.length === 0) throw new FillError('page-absent', 'no page matches the requested URL', phase);
  if (matches.length > 1) throw new FillError('page-ambiguous', `${matches.length} pages match the requested URL`, phase);
  return matches[0];
}

async function resolveFrameChain(page, options, deadline, phase) {
  let frame = page.mainFrame();
  const chain = [];
  for (const selector of options.frameSelectors) {
    const locator = frame.locator(selector);
    const count = await locator.count();
    if (count === 0) throw new FillError('frame-absent', 'a frame selector matched no element', phase);
    if (count > 1) throw new FillError('frame-ambiguous', `a frame selector matched ${count} elements`, phase);
    const handle = await locator.elementHandle({ timeout: deadline.budget(phase) });
    const tag = await handle.evaluate((element) => element.tagName);
    if (tag !== 'IFRAME' && tag !== 'FRAME') {
      throw new FillError('frame-invalid', `a frame selector matched a ${tag.toLowerCase()} element`, phase);
    }
    const child = await handle.contentFrame();
    if (!child) throw new FillError('frame-invalid', 'a frame selector matched an iframe without a content frame', phase);
    frame = child;
    chain.push({ url: frame.url() });
  }
  return { frame, chain };
}

async function findElement(frame, options, deadline, phase) {
  const locator = frame.locator(options.selector);
  const count = await locator.count();
  if (count === 0) throw new FillError('target-absent', 'the selector matched no element', phase);
  if (count > 1) throw new FillError('target-ambiguous', `the selector matched ${count} elements`, phase);
  const handle = await locator.elementHandle({ timeout: deadline.budget(phase) });
  const kind = await handle.evaluate((element) => ({
    tag: element.tagName.toLowerCase(),
    inputType: element.tagName === 'INPUT' ? element.type.toLowerCase() : null,
    contentEditable: element.isContentEditable === true,
  }));
  const fillable =
    (kind.tag === 'input' && FILLABLE_INPUT_TYPES.has(kind.inputType)) ||
    kind.tag === 'textarea' ||
    (kind.tag !== 'input' && kind.contentEditable);
  if (!fillable) throw new FillError('target-unfillable', 'the element is not a fillable text control', phase);
  const timeout = deadline.budget(phase);
  if (!(await handle.isVisible({ timeout }))) throw new FillError('target-hidden', 'the element is not visible', phase);
  if (!(await handle.isEnabled({ timeout }))) throw new FillError('target-disabled', 'the element is disabled', phase);
  if (!(await handle.isEditable({ timeout }))) throw new FillError('target-readonly', 'the element is read-only', phase);
  return { handle, kind };
}

async function prepare(browser, options, deadline, phase) {
  const selected = await selectPage(browser, options, deadline, phase);
  const { frame, chain } = await resolveFrameChain(selected.page, options, deadline, phase);
  const { handle, kind } = await findElement(frame, options, deadline, phase);
  return { selected, chain, handle, kind };
}

function sleep(ms) {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

async function run(options, value) {
  const deadline = new Deadline(options.timeoutMs);
  const versions = { browser: null, protocol: null, library: LIBRARY_VERSION };
  let browser;
  let phase = 'connect';
  try {
    try {
      browser = await chromium.connectOverCDP(options.endpoint, { timeout: deadline.budget(phase) });
    } catch (error) {
      if (isTimeoutError(error)) throw new FillError('timeout', 'connection timed out', phase);
      throw new FillError('connect-failed', 'could not connect to the CDP endpoint', phase);
    }

    phase = 'version';
    versions.browser = browser.version();
    {
      const session = await browser.newBrowserCDPSession();
      try {
        const info = await session.send('Browser.getVersion');
        versions.browser = info.product;
        versions.protocol = info.protocolVersion;
      } finally {
        await session.detach().catch(() => {});
      }
    }

    phase = 'prepare';
    const prepared = await prepare(browser, options, deadline, phase);

    phase = 'delay';
    if (options.delayBeforeInsertMs > 0) {
      await sleep(Math.min(options.delayBeforeInsertMs, deadline.remaining()));
    }
    deadline.budget(phase);

    phase = 'revalidate';
    // First: is the prepared element still the live element in its document?
    // A navigation, a removed frame, or a detached node means the prepared
    // target is gone, which is a target change regardless of what a fresh
    // selection would now find.
    let connected = false;
    try {
      connected = await prepared.handle.evaluate((element) => element.isConnected === true);
    } catch (error) {
      if (!isDetachedError(error) && !isTimeoutError(error)) throw error;
    }
    if (!connected || prepared.selected.page.isClosed()) {
      throw new FillError('target-changed', 'the target changed before insertion', phase);
    }
    // Second: repeat the whole selection so a newly ambiguous page, frame, or
    // selector is reported as such, and the result must be the same node.
    let again;
    try {
      again = await prepare(browser, options, deadline, phase);
    } catch (error) {
      if (error instanceof FillError) throw error;
      if (isDetachedError(error)) throw new FillError('target-changed', 'the target changed before insertion', phase);
      throw error;
    }
    let same = false;
    try {
      same = await prepared.handle.evaluate((a, b) => a === b, again.handle);
    } catch (error) {
      if (isDetachedError(error) || isTimeoutError(error)) same = false;
      else throw error;
    }
    if (!same || again.selected.page !== prepared.selected.page) {
      throw new FillError('target-changed', 'the target changed before insertion', phase);
    }

    phase = 'fill';
    try {
      await prepared.handle.fill(value, { timeout: deadline.budget(phase), force: false });
    } catch (error) {
      if (isTimeoutError(error)) throw new FillError('timeout', 'fill timed out', phase);
      if (isDetachedError(error)) throw new FillError('target-changed', 'the target changed during insertion', phase);
      throw new FillError('target-changed', 'fill failed', phase);
    }

    return {
      version: 1,
      implementation: 'reference',
      outcome: 'filled',
      reason: null,
      message: '',
      details: {
        page: { url: prepared.selected.url, contextIndex: prepared.selected.contextIndex },
        frameChain: prepared.chain,
        elementTag: prepared.kind.tag,
        inputType: prepared.kind.inputType,
      },
      versions,
    };
  } catch (error) {
    let reason;
    let message;
    if (error instanceof FillError) {
      reason = error.reason;
      message = error.message;
      phase = error.phase ?? phase;
    } else if (isTimeoutError(error)) {
      reason = 'timeout';
      message = 'operation deadline expired';
    } else if (isDetachedError(error)) {
      reason = 'target-changed';
      message = 'the target changed';
    } else if (phase === 'connect') {
      reason = 'connect-failed';
      message = 'could not connect to the CDP endpoint';
    } else {
      reason = 'target-changed';
      message = 'unexpected failure';
    }
    process.stderr.write(`reference-fill: ${reason}: ${message} (${phase})\n`);
    return {
      version: 1,
      implementation: 'reference',
      outcome: 'error',
      reason,
      message,
      details: { phase },
      versions,
    };
  } finally {
    if (browser) {
      // For a connectOverCDP browser, close() disconnects this client only.
      await browser.close().catch(() => {});
    }
  }
}

async function readStdin() {
  const chunks = [];
  for await (const chunk of process.stdin) chunks.push(chunk);
  return Buffer.concat(chunks).toString('utf8');
}

const options = parseArgs(process.argv.slice(2));
const value = await readStdin();
const result = await run(options, value);
process.stdout.write(`${JSON.stringify(result)}\n`);
process.exit(result.outcome === 'filled' ? 0 : 8);
