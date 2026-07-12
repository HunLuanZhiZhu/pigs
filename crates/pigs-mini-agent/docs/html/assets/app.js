/* ============================================================
   Pigs Mini Agent — Terminal Codex
   轻量交互：侧边栏、当前页高亮、键盘导航
   ============================================================ */

(function () {
  'use strict';

  // --- 侧边栏移动端切换 ---
  const sidebar = document.querySelector('.sidebar');
  const toggle = document.querySelector('.menu-toggle');

  if (toggle && sidebar) {
    toggle.addEventListener('click', () => {
      sidebar.classList.toggle('open');
      toggle.classList.toggle('open');
    });

    // 点击导航链接后关闭侧边栏（移动端）
    sidebar.querySelectorAll('.nav a').forEach((link) => {
      link.addEventListener('click', () => {
        if (window.innerWidth <= 860) {
          sidebar.classList.remove('open');
          toggle.classList.remove('open');
        }
      });
    });

    // 点击主内容区关闭侧边栏
    document.querySelector('.main')?.addEventListener('click', () => {
      if (sidebar.classList.contains('open')) {
        sidebar.classList.remove('open');
        toggle.classList.remove('open');
      }
    });
  }

  // --- 当前页高亮 ---
  const path = window.location.pathname.split('/').pop() || 'index.html';
  document.querySelectorAll('.nav a').forEach((link) => {
    const href = link.getAttribute('href');
    if (href === path || (path === '' && href === 'index.html')) {
      link.classList.add('active');
    }
  });

  // --- 键盘导航：← → 翻页 ---
  const prev = document.querySelector('.page-nav a.prev');
  const next = document.querySelector('.page-nav a.next');

  document.addEventListener('keydown', (e) => {
    // 输入框中不拦截
    if (e.target.matches('input, textarea')) return;

    if (e.key === 'ArrowLeft' && prev) {
      window.location.href = prev.getAttribute('href');
    } else if (e.key === 'ArrowRight' && next) {
      window.location.href = next.getAttribute('href');
    }
  });

  // --- 代码块复制按钮 ---
  document.querySelectorAll('pre').forEach((pre) => {
    // 跳过已有按钮的
    if (pre.querySelector('.copy-btn')) return;

    const btn = document.createElement('button');
    btn.className = 'copy-btn';
    btn.textContent = 'copy';
    btn.setAttribute('aria-label', '复制代码');
    btn.style.cssText = [
      'position:absolute',
      'top:0.5rem',
      'right:0.6rem',
      'font-family:var(--font-mono)',
      'font-size:0.6rem',
      'letter-spacing:0.08em',
      'text-transform:uppercase',
      'background:var(--ink-800)',
      'color:var(--ink-400)',
      'border:1px solid var(--ink-700)',
      'border-radius:3px',
      'padding:0.2rem 0.5rem',
      'cursor:pointer',
      'opacity:0',
      'transition:opacity 0.15s, color 0.15s, border-color 0.15s',
      'z-index:2',
    ].join(';');

    pre.style.position = 'relative';
    pre.appendChild(btn);

    pre.addEventListener('mouseenter', () => { btn.style.opacity = '1'; });
    pre.addEventListener('mouseleave', () => { btn.style.opacity = '0'; });

    btn.addEventListener('click', async () => {
      const code = pre.querySelector('code');
      const text = code ? code.textContent : pre.textContent.replace(/copy$/, '').trim();
      try {
        await navigator.clipboard.writeText(text);
        btn.textContent = 'copied';
        btn.style.color = 'var(--term)';
        btn.style.borderColor = 'rgba(94,207,138,0.4)';
        setTimeout(() => {
          btn.textContent = 'copy';
          btn.style.color = 'var(--ink-400)';
          btn.style.borderColor = 'var(--ink-700)';
        }, 1500);
      } catch (_) {
        btn.textContent = 'fail';
      }
    });
  });
})();
