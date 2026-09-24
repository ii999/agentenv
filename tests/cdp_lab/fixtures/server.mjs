// Fixture server for the CDP filling parity lab.
//
// Serves the scenario pages on two loopback origins so that a cross-origin
// iframe becomes an out-of-process iframe under Chromium site isolation:
//   origin A: http://127.0.0.1:<port>
//   origin B: http://localhost:<port>
// Pages are templates; {{SELF}} and {{OTHER}} are replaced with the origin
// serving the request and the opposite origin.
//
// Every instrumented field posts its current value and recorded event
// sequence to /receipt. The harness reads receipts to verify exact delivery
// from the fixture side without the filling client ever reading the field.

import { createServer } from 'node:http';
import { readFile } from 'node:fs/promises';
import { dirname, join, normalize } from 'node:path';
import { fileURLToPath } from 'node:url';

const here = dirname(fileURLToPath(import.meta.url));
const pagesDir = join(here, 'pages');
const vendor = {
  '/vendor/react.js': join(here, 'node_modules/react/umd/react.production.min.js'),
  '/vendor/react-dom.js': join(here, 'node_modules/react-dom/umd/react-dom.production.min.js'),
};

const MAX_BODY = 64 * 1024;

export async function startFixtureServer({ port = 0 } = {}) {
  const receipts = new Map();
  const state = { receipts, port: 0, originA: '', originB: '' };

  const handler = async (request, response) => {
    try {
      await route(request, response, state);
    } catch (error) {
      response.writeHead(500, { 'content-type': 'text/plain' });
      response.end(`fixture server error: ${error?.message ?? error}`);
    }
  };

  const v4 = createServer(handler);
  await new Promise((resolve, reject) => {
    v4.once('error', reject);
    v4.listen(port, '127.0.0.1', resolve);
  });
  state.port = v4.address().port;
  const v6 = createServer(handler);
  let v6Bound = true;
  await new Promise((resolve) => {
    v6.once('error', () => {
      v6Bound = false;
      resolve();
    });
    v6.listen(state.port, '::1', resolve);
  });
  state.originA = `http://127.0.0.1:${state.port}`;
  state.originB = `http://localhost:${state.port}`;

  return {
    port: state.port,
    originA: state.originA,
    originB: state.originB,
    ipv6: v6Bound,
    receipts,
    async close() {
      await Promise.all([
        new Promise((resolve) => v4.close(resolve)),
        v6Bound ? new Promise((resolve) => v6.close(resolve)) : Promise.resolve(),
      ]);
    },
  };
}

async function route(request, response, state) {
  const url = new URL(request.url, 'http://fixture.invalid');
  const host = request.headers.host ?? '';
  const self = host.startsWith('localhost') ? state.originB : state.originA;
  const other = self === state.originA ? state.originB : state.originA;

  if (url.pathname === '/health') {
    return json(response, 200, { ok: true, originA: state.originA, originB: state.originB });
  }

  if (url.pathname === '/receipt') {
    if (request.method === 'POST') {
      const body = await readBody(request);
      const receipt = JSON.parse(body);
      if (typeof receipt.instance !== 'string' || typeof receipt.field !== 'string') {
        return json(response, 400, { error: 'receipt requires instance and field' });
      }
      state.receipts.set(`${receipt.instance}:${receipt.field}`, {
        ...receipt,
        origin: self,
        at: Date.now(),
      });
      return json(response, 204, null);
    }
    if (request.method === 'GET') {
      return json(response, 200, { receipts: [...state.receipts.values()] });
    }
    if (request.method === 'DELETE') {
      state.receipts.clear();
      return json(response, 204, null);
    }
    return json(response, 405, { error: 'method not allowed' });
  }

  if (url.pathname === '/fixture.js' || url.pathname === '/worker.js' || url.pathname === '/sw.js') {
    const source = await readFile(join(here, url.pathname.slice(1)));
    response.writeHead(200, { 'content-type': 'text/javascript', 'cache-control': 'no-store' });
    return response.end(source);
  }

  if (vendor[url.pathname]) {
    const source = await readFile(vendor[url.pathname]);
    response.writeHead(200, { 'content-type': 'text/javascript' });
    return response.end(source);
  }

  if (url.pathname.startsWith('/pages/')) {
    const name = normalize(url.pathname.slice('/pages/'.length));
    if (name.includes('..') || name.includes('/') || !name.endsWith('.html')) {
      return json(response, 404, { error: 'unknown page' });
    }
    let template;
    try {
      template = await readFile(join(pagesDir, name), 'utf8');
    } catch {
      return json(response, 404, { error: 'unknown page' });
    }
    const page = template
      .replaceAll('{{SELF}}', self)
      .replaceAll('{{OTHER}}', other)
      .replaceAll('{{QUERY}}', url.search);
    response.writeHead(200, { 'content-type': 'text/html; charset=utf-8', 'cache-control': 'no-store' });
    return response.end(page);
  }

  return json(response, 404, { error: 'not found' });
}

function json(response, status, body) {
  const headers = {
    'access-control-allow-origin': '*',
    'access-control-allow-methods': 'GET, POST, DELETE',
    'access-control-allow-headers': 'content-type',
  };
  if (body === null) {
    response.writeHead(status, headers);
    return response.end();
  }
  response.writeHead(status, { ...headers, 'content-type': 'application/json' });
  response.end(JSON.stringify(body));
}

function readBody(request) {
  return new Promise((resolve, reject) => {
    let size = 0;
    const chunks = [];
    request.on('data', (chunk) => {
      size += chunk.length;
      if (size > MAX_BODY) {
        reject(new Error('body too large'));
        request.destroy();
        return;
      }
      chunks.push(chunk);
    });
    request.on('end', () => resolve(Buffer.concat(chunks).toString('utf8')));
    request.on('error', reject);
  });
}

if (process.argv[1] && fileURLToPath(import.meta.url) === process.argv[1]) {
  const portIndex = process.argv.indexOf('--port');
  const port = portIndex === -1 ? 0 : Number(process.argv[portIndex + 1]);
  const server = await startFixtureServer({ port });
  process.stdout.write(
    `${JSON.stringify({ port: server.port, originA: server.originA, originB: server.originB, ipv6: server.ipv6 })}\n`,
  );
  const stop = () => server.close().then(() => process.exit(0));
  process.stdin.on('end', stop);
  process.stdin.on('close', stop);
  process.on('SIGTERM', stop);
  process.on('SIGINT', stop);
  process.stdin.resume();
}
