import { api } from '../api.js';
import { esc } from '../ui.js';

export function render(root, { path, query, session }) {
  const mode = session.info?.signup_mode || 'invite';
  const isSignup = path === '/signup' || (query.invite && path !== '/login');
  const app = esc(session.info?.app_name || 'Encore');
  root.innerHTML = `
    <div class="auth-wrap">
      <div class="auth-card">
        <div class="brand"><img src="/favicon.svg" alt="">${app}</div>
        <p class="muted" style="text-align:center;margin:0">${isSignup ? 'Create your account' : 'Sign in to request and stream music'}</p>
        <div id="auth-err" class="error-box hidden" style="margin-top:16px"></div>
        ${isSignup ? signupForm(mode, query.invite) : loginForm()}
        <p class="small muted" style="text-align:center;margin:18px 0 0">
          ${isSignup ? `Already have an account? <a href="#/login" style="color:var(--accent)">Sign in</a>`
            : mode === 'closed' ? 'Sign-ups are closed. Ask the admin for an account.'
            : `New here? <a href="#/signup" style="color:var(--accent)">${mode === 'invite' ? 'Sign up with an invite' : 'Create an account'}</a>`}
        </p>
      </div>
    </div>`;
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
      err.textContent = ex.message;
      err.classList.remove('hidden');
      btn.disabled = false;
    }
  });
}

function loginForm() {
  return `<form autocomplete="on">
    <label class="field"><span>Username</span><input class="input" name="username" autocomplete="username" required autofocus></label>
    <label class="field"><span>Password</span><input class="input" type="password" name="password" autocomplete="current-password" required></label>
    <button class="btn primary" type="submit" style="padding:12px">Sign in</button>
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
