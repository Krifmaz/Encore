export class ApiError extends Error {
  constructor(status, message, data = null) { super(message); this.status = status; this.data = data; }
}

/** Album id → its cover's main colour ("#rrggbb", the server's `cover_color`), painted while the cover loads. */
export const coverColors = new Map();

const isAlbumId = (s) => typeof s === 'string' && /^[0-9a-f]{16}$/i.test(s);

/** Records every `cover_color` in a JSON answer, under the album whose cover the object shows. */
function noteCoverColors(v) {
  if (Array.isArray(v)) { v.forEach(noteCoverColors); return; }
  if (!v || typeof v !== 'object') return;
  const c = v.cover_color;
  if (typeof c === 'string' && /^#[0-9a-f]{6}$/i.test(c)) {
    const id = v.album_id || v.cover_album || (isAlbumId(v.id) && ('album' in v || 'albumartist' in v) ? v.id : '');
    if (id) coverColors.set(id, c);
  }
  for (const k in v) { const x = v[k]; if (x && typeof x === 'object') noteCoverColors(x); }
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
  if (res.ok && text.includes('"cover_color"')) noteCoverColors(data);
  if (!res.ok) {
    if (res.status === 401 && !path.startsWith('/auth/')) {
      window.dispatchEvent(new CustomEvent('encore:unauthorized'));
    }
    let msg = (data && data.detail) || res.statusText || 'Request failed';
    if (Array.isArray(msg)) msg = msg.map(m => m.msg).join(', ');
    throw new ApiError(res.status, msg, data);
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
