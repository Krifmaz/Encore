// The library's side scrubber: section labels from GET /api/library/index on the right edge (desktop: on
// hover; touch: while scrolling, then drag), a bubble with the label under the finger, and a jump to the
// section's first row through the page's own paging. web-rs/src/scrubber.rs builds the same DOM.
import { esc } from './ui.js';

/** Pixels a rail label needs; with more sections than fit, every Nth label is shown (all still jump). */
const LABEL_PX = 18;

/**
 * Mounts the scrubber in `view`. `rows()` → the list's row elements in list order; `long()` → whether the
 * list is long enough to need it; `jump(offset)` → loads up to that row and scrolls to it (a promise).
 * Returns { set(sections) } (sections: [{label, offset}]; [] hides it). `onLeave` gets the cleanup.
 */
export function scrubber(view, { rows, long, jump, onLeave }) {
  const el = document.createElement('div');
  el.className = 'scrubber';
  el.id = 'lib-scrub';
  el.hidden = true;
  el.innerHTML = '<div class="scrub-rail"></div><div class="scrub-bubble"></div>';
  view.appendChild(el);
  const rail = el.querySelector('.scrub-rail'), bubble = el.querySelector('.scrub-bubble');
  let sections = [], cur = -1, dragging = false, fade = 0, frame = 0, picked = -1, chain = Promise.resolve();

  const paint = () => {
    if (el.hidden) return;
    const h = rail.clientHeight || 1;
    const step = Math.max(1, Math.ceil(sections.length * LABEL_PX / h));
    rail.querySelectorAll('[data-s]').forEach((s, i) => s.classList.toggle('skip', i % step !== 0 && i !== sections.length - 1));
  };
  const visible = () => { el.hidden = sections.length < 2 || !long(); paint(); };
  // the section the top of the screen is in
  const track = () => {
    frame = 0;
    if (el.hidden || dragging) return;
    const list = rows();
    let at = 0;
    for (let i = 0; i < sections.length; i++) {
      const r = list[sections[i].offset];
      if (!r || r.getBoundingClientRect().top > 120) break;
      at = i;
    }
    mark(at);
  };
  const mark = (i) => {
    if (i === cur) return;
    rail.querySelector('.cur')?.classList.remove('cur');
    rail.querySelector(`[data-s="${i}"]`)?.classList.add('cur');
    cur = i;
  };
  const onScroll = () => {
    if (el.hidden) return;
    el.classList.add('on');
    clearTimeout(fade);
    fade = setTimeout(() => { if (!dragging) el.classList.remove('on'); }, 1500);
    if (!frame) frame = requestAnimationFrame(track);
  };
  const pick = (e) => {
    const r = rail.getBoundingClientRect();
    const y = Math.min(Math.max(e.clientY - r.top, 0), r.height - 1);
    const i = Math.min(sections.length - 1, Math.floor(y / r.height * sections.length));
    bubble.style.top = `${y + rail.offsetTop}px`;
    if (i === picked) return;
    picked = i;
    bubble.textContent = sections[i].label;
    mark(i);
    if (e.pointerType === 'touch') navigator.vibrate?.(4);   // a tick per section
    const offset = sections[i].offset;
    chain = chain.then(() => (picked === i ? jump(offset) : null)).catch(() => {});   // the latest pick wins
  };
  rail.addEventListener('pointerdown', (e) => {
    if (el.hidden) return;
    e.preventDefault();
    dragging = true;
    picked = -1;
    el.classList.add('drag', 'on');
    try { rail.setPointerCapture(e.pointerId); } catch { /* ignore */ }
    pick(e);
  });
  rail.addEventListener('pointermove', (e) => { if (dragging) pick(e); });
  const end = () => {
    if (!dragging) return;
    dragging = false;
    el.classList.remove('drag');
    onScroll();
  };
  rail.addEventListener('pointerup', end);
  rail.addEventListener('pointercancel', end);
  window.addEventListener('scroll', onScroll, { passive: true });
  window.addEventListener('resize', visible);
  onLeave?.(() => {
    window.removeEventListener('scroll', onScroll);
    window.removeEventListener('resize', visible);
    clearTimeout(fade);
    el.remove();
  });
  return {
    set(next) {
      sections = Array.isArray(next) ? next : [];
      cur = -1;
      rail.innerHTML = sections.map((s, i) => `<span data-s="${i}">${esc(s.label)}</span>`).join('');
      visible();
      track();
    },
    refresh: visible,
  };
}
