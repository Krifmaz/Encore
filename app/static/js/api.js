export class ApiError extends Error {
  constructor(status, message) { super(message); this.status = status; }
}

async function request(method, path, body) {
  const opts = { method, headers: { 'X-Requested-With': 'encore' }, credentials: 'same-origin' };
  if (body !== undefined) {
    opts.headers['Content-Type'] = 'application/json';
    opts.body = JSON.stringify(body);
  }
  let res;
  try {
    res = await fetch('/api' + path, opts);
  } catch (e) {
    window.dispatchEvent(new CustomEvent('encore:server-down'));
    throw new ApiError(0, 'Not connected to the server');
  }
  window.dispatchEvent(new CustomEvent(res.status >= 502 && res.status <= 504 ? 'encore:server-down' : 'encore:server-up'));
  let data = null;
  const text = await res.text();
  try { data = text ? JSON.parse(text) : null; } catch { data = text; }
  if (!res.ok) {
    if (res.status === 401 && !path.startsWith('/auth/')) {
      window.dispatchEvent(new CustomEvent('encore:unauthorized'));
    }
    let msg = (data && data.detail) || res.statusText || 'Request failed';
    if (Array.isArray(msg)) msg = msg.map(m => m.msg).join(', ');
    throw new ApiError(res.status, msg);
  }
  return data;
}

export const api = {
  get: (p) => request('GET', p),
  post: (p, b = {}) => request('POST', p, b),
  put: (p, b = {}) => request('PUT', p, b),
  patch: (p, b = {}) => request('PATCH', p, b),
  del: (p) => request('DELETE', p),
};
