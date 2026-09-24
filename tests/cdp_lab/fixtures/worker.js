// Dedicated worker for the workers fixture page: echoes messages and keeps
// a timer alive so the worker target stays attached during the fill.
self.onmessage = (event) => self.postMessage(`pong:${event.data}`);
setInterval(() => {}, 1000);
