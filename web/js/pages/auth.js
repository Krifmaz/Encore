import { api } from '../api.js';
import { logo } from '../appearance.js';
import { esc } from '../ui.js';

function resetPage(root, query, app) {
  root.innerHTML = `<div class="auth-wrap"><div class="auth-card">
      <div class="brand">${logo()}${app}</div>
      <p class="muted" style="text-align:center;margin:0">Choose a new password</p>
      <form>
        <label class="field"><span>New password</span><input class="input" type="password" name="password" autocomplete="new-password" minlength="8" required autofocus></label>
        <label class="field"><span>Repeat it</span><input class="input" type="password" name="password2" autocomplete="new-password" required></label>
        <div class="error-box hidden" id="auth-err"></div>
        <button class="btn primary" type="submit" style="padding:12px">Save new password</button>
      </form></div></div>`;
  const form = root.querySelector('form'), err = root.querySelector('#auth-err');
  form.onsubmit = async (e) => {
    e.preventDefault();
    const d = Object.fromEntries(new FormData(form));
    err.classList.add('hidden');
    try {
      if (d.password !== d.password2) throw new Error('Passwords do not match');
      await api.post('/auth/reset', { token: query.token || '', password: d.password });
      form.outerHTML = '<div class="test-result ok"><span>Password changed – sign in with your new password.</span></div><a class="btn primary" href="#/login" style="justify-content:center">Sign in</a>';
    } catch (ex) { err.textContent = ex.message; err.classList.remove('hidden'); }
  };
}

function forgotForm(root) {
  const card = root.querySelector('.auth-card form');
  card.outerHTML = `<form id="forgot">
    <p class="small muted" style="margin:0">Enter your username or email. If your account has an email address, we'll send a reset link.</p>
    <label class="field"><span>Username or email</span><input class="input" name="login" required autofocus></label>
    <div class="error-box hidden" id="forgot-err"></div>
    <button class="btn primary" type="submit" style="padding:12px">Send reset link</button>
    <a href="#/login" class="small muted" style="text-align:center" onclick="location.reload()">Back to sign in</a></form>`;
  const f = root.querySelector('#forgot'), err = root.querySelector('#forgot-err');
  f.onsubmit = async (e) => {
    e.preventDefault();
    err.classList.add('hidden');
    try {
      const r = await api.post('/auth/forgot', Object.fromEntries(new FormData(f)));
      f.innerHTML = `<div class="test-result ok"><span>${esc(r.message)}</span></div>`;
    } catch (ex) { err.textContent = ex.message; err.classList.remove('hidden'); }
  };
}

/** Sign-in link from an email. Accounts with two-step sign-in also enter their code. */
function magicPage(root, query, app) {
  root.innerHTML = `<div class="auth-wrap"><div class="auth-card">
      <div class="brand">${logo()}${app}</div>
      <p class="muted" style="text-align:center;margin:0" id="magic-msg">Signing you in…</p>
      <form class="hidden" id="magic-code"><label class="field"><span>Code from your authenticator app</span>
        <input class="input" name="code" inputmode="numeric" autocomplete="one-time-code" maxlength="8" required></label>
        <button class="btn primary" type="submit" style="padding:12px">Sign in</button></form>
      <div class="error-box hidden" id="auth-err"></div></div></div>`;
  const err = root.querySelector('#auth-err'), form = root.querySelector('#magic-code');
  const go = async (code) => {
    err.classList.add('hidden');
    try {
      await api.post('/auth/magic', { token: query.token || '', code });
      location.hash = '#/';
      location.reload();
    } catch (ex) {
      if (ex.data?.two_factor) {
        form.classList.remove('hidden');
        root.querySelector('#magic-msg').textContent = 'One more step';
        if (code) { err.textContent = ex.message; err.classList.remove('hidden'); }
        form.code.focus();
        return;
      }
      root.querySelector('#magic-msg').textContent = '';
      err.innerHTML = `${esc(ex.message)} – <a href="#/login" onclick="location.reload()">sign in another way</a>`;
      err.classList.remove('hidden');
    }
  };
  form.onsubmit = (e) => { e.preventDefault(); go(form.code.value); };
  go(undefined);
}

function emailLinkForm(root) {
  const card = root.querySelector('.auth-card form');
  card.outerHTML = `<form id="email-link">
    <p class="small muted" style="margin:0">We'll email you a link that signs you in – no password needed.</p>
    <label class="field"><span>Username or email</span><input class="input" name="login" required autofocus></label>
    <div class="error-box hidden" id="link-err"></div>
    <button class="btn primary" type="submit" style="padding:12px">Email me a sign-in link</button>
    <a href="#/login" class="small muted" style="text-align:center" onclick="location.reload()">Back to sign in</a></form>`;
  const f = root.querySelector('#email-link'), err = root.querySelector('#link-err');
  f.onsubmit = async (e) => {
    e.preventDefault();
    err.classList.add('hidden');
    try {
      const r = await api.post('/auth/email-link', Object.fromEntries(new FormData(f)));
      f.innerHTML = `<div class="test-result ok"><span>${esc(r.message)}</span></div>`;
    } catch (ex) { err.textContent = ex.message; err.classList.remove('hidden'); }
  };
}

export function render(root, { path, query, session }) {
  if (path === '/reset') return resetPage(root, query, esc(session.info?.app_name || 'Encore'));
  if (path === '/magic') return magicPage(root, query, esc(session.info?.app_name || 'Encore'));
  const mode = session.info?.signup_mode || 'invite';
  const isSignup = path === '/signup' || (query.invite && path !== '/login');
  const app = esc(session.info?.app_name || 'Encore');
  root.innerHTML = `
    <div class="auth-wrap">
      <div class="auth-card">
        <div class="brand">${logo()}${app}</div>
        <p class="muted" style="text-align:center;margin:0">${isSignup ? 'Create your account' : 'Sign in to request music and manage the library'}</p>
        <div id="auth-err" class="error-box hidden" style="margin-top:16px"></div>
        ${isSignup ? signupForm(mode, query.invite) : loginForm(session.info?.email_login)}
        <p class="small muted" style="text-align:center;margin:18px 0 0">
          ${isSignup ? `Already have an account? <a href="#/login" style="color:var(--accent)">Sign in</a>`
            : mode === 'closed' ? 'Sign-ups are closed. Ask the admin for an account.'
            : `New here? <a href="#/signup" style="color:var(--accent)">${mode === 'invite' ? 'Sign up with an invite' : 'Create an account'}</a>`}
        </p>
        ${window.EncoreNative?.changeServer ? `<p class="small muted" style="text-align:center;margin:10px 0 0"><a href="#" id="change-server" style="color:var(--accent)">Change server</a></p>` : ''}
      </div>
    </div>`;
  root.querySelector('#forgot-link')?.addEventListener('click', (e) => { e.preventDefault(); forgotForm(root); });
  root.querySelector('#email-link-btn')?.addEventListener('click', (e) => { e.preventDefault(); emailLinkForm(root); });
  root.querySelector('#change-server')?.addEventListener('click', (e) => { e.preventDefault(); window.EncoreNative.changeServer(); });
  const form = root.querySelector('form');
  if (!form) return;
  const err = root.querySelector('#auth-err');
  form.addEventListener('submit', async (e) => {
    e.preventDefault();
    const data = Object.fromEntries(new FormData(form));
    const btn = form.querySelector('button[type=submit]');
    btn.disabled = true;
    err.classList.add('hidden');
    try {
      if (isSignup) {
        if (data.password !== data.password2) throw new Error('Passwords do not match');
        delete data.password2;
        await api.post('/auth/signup', data);
      } else {
        await api.post('/auth/login', data);
      }
      location.hash = '#/';
      location.reload();
    } catch (ex) {
      btn.disabled = false;
      if (ex.data?.two_factor && !form.querySelector('[name=code]')) {
        // two-step sign-in: ask for the authenticator code, keep what was typed
        btn.insertAdjacentHTML('beforebegin', `<label class="field"><span>Code from your authenticator app</span>
          <input class="input" name="code" inputmode="numeric" autocomplete="one-time-code" maxlength="8" required></label>`);
        form.querySelector('[name=code]').focus();
        return;
      }
      err.textContent = ex.message;
      err.classList.remove('hidden');
    }
  });
}

function loginForm(emailLogin) {
  return `<form autocomplete="on">
    <label class="field"><span>Username</span><input class="input" name="username" autocomplete="username" required autofocus></label>
    <label class="field"><span>Password</span><input class="input" type="password" name="password" autocomplete="current-password" required></label>
    <button class="btn primary" type="submit" style="padding:12px">Sign in</button>
    ${emailLogin ? '<button type="button" class="btn" id="email-link-btn" style="padding:12px">Email me a sign-in link</button>' : ''}
    <a href="#" id="forgot-link" class="small muted" style="text-align:center">Forgot password?</a>
  </form>`;
}

function signupForm(mode, invite) {
  if (mode === 'closed') return '<div class="error-box" style="margin-top:18px">Sign-ups are currently closed.</div>';
  return `<form autocomplete="on">
    ${mode === 'invite' || invite ? `<label class="field"><span>Invite code</span><input class="input mono" name="invite" value="${esc(invite || '')}" placeholder="XXXX-XXXX-XXXX" required></label>` : ''}
    <label class="field"><span>Username</span><input class="input" name="username" autocomplete="username" minlength="3" maxlength="32" required></label>
    <label class="field"><span>Email <small>(optional)</small></span><input class="input" type="email" name="email" autocomplete="email"></label>
    <label class="field"><span>Password</span><input class="input" type="password" name="password" autocomplete="new-password" minlength="8" required></label>
    <label class="field"><span>Confirm password</span><input class="input" type="password" name="password2" autocomplete="new-password" minlength="8" required></label>
    <button class="btn primary" type="submit" style="padding:12px">Create account</button>
  </form>`;
}
