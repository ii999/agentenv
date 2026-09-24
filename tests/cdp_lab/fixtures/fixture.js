// Page instrumentation for the CDP filling parity lab.
//
// Records the event sequence delivered to every instrumented field and posts
// the field's current value to the fixture server on each input or change
// event. The filling clients never read fields; the harness verifies delivery
// through these receipts and through the host session's own snapshot.
(function () {
  const EVENTS = ['focus', 'beforeinput', 'input', 'change', 'keydown', 'keyup', 'blur'];
  const instance = `${document.body.dataset.page}:${Math.random().toString(36).slice(2, 10)}`;
  const events = {};
  const values = {};

  function valueOf(element) {
    if (element.isContentEditable) return element.textContent;
    return element.value;
  }

  function post(field, value, valueSource) {
    const payload = {
      instance,
      page: document.body.dataset.page,
      frame: document.body.dataset.frame || 'main',
      field,
      value,
      valueSource: valueSource || 'dom',
      events: events[field].slice(),
      href: location.href,
    };
    values[field] = value;
    try {
      fetch('/receipt', {
        method: 'POST',
        headers: { 'content-type': 'application/json' },
        body: JSON.stringify(payload),
        keepalive: true,
      });
    } catch (_) {
      // Reporting failures are visible as missing receipts.
    }
  }

  function attach(element) {
    const field = element.id;
    if (!field || events[field]) return;
    events[field] = [];
    for (const name of EVENTS) {
      element.addEventListener(name, (event) => {
        events[field].push(name + (event.isTrusted ? '' : '!untrusted'));
        if (name === 'input' || name === 'change') {
          post(field, valueOf(element));
        }
      });
    }
  }

  function attachAll() {
    document.querySelectorAll('input, textarea, [contenteditable]').forEach(attach);
  }

  window.__fixture = {
    instance,
    events,
    values,
    attach,
    attachAll,
    report: post,
    snapshot() {
      const fields = {};
      document.querySelectorAll('input, textarea, [contenteditable]').forEach((element) => {
        if (element.id) fields[element.id] = valueOf(element);
      });
      return { instance, href: location.href, fields, events };
    },
  };
  attachAll();
})();
