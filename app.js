/**
 * TiaRun — Official Website Interactive Controller
 * Architecture: Vanilla JavaScript (Zero Dependencies, ES6+)
 * Performance: Lightweight, Non-blocking, Progressive Enhancement
 */

(function () {
  'use strict';

  const GITHUB_REPO = 'hieunx1024/tiarun';

  // Global state for detected OS and latest release
  let currentDetectedOS = null;
  let currentLatestVersion = 'v1.0.0';
  const directDownloadUrls = {};

  /* ==========================================================================
     1. Global Site Theme Controller (Dark / Light)
     ========================================================================== */
  function initSiteTheme() {
    const toggleBtn = document.getElementById('theme-toggle-btn');
    const root = document.documentElement;
    const STORAGE_KEY = 'tiarun-site-theme';

    const savedTheme = localStorage.getItem(STORAGE_KEY);
    const prefersDark = window.matchMedia('(prefers-color-scheme: dark)').matches;
    const initialTheme = savedTheme || (prefersDark ? 'dark' : 'light');

    function applyTheme(theme) {
      root.setAttribute('data-theme', theme);
      if (toggleBtn) {
        const isDark = theme === 'dark';
        toggleBtn.setAttribute('aria-label', isDark ? 'Chuyển sang chế độ Sáng' : 'Chuyển sang chế độ Tối');
        toggleBtn.setAttribute('title', isDark ? 'Chuyển sang chế độ Sáng' : 'Chuyển sang chế độ Tối');
      }
    }

    applyTheme(initialTheme);

    if (toggleBtn) {
      toggleBtn.addEventListener('click', () => {
        const currentTheme = root.getAttribute('data-theme') || 'dark';
        const nextTheme = currentTheme === 'dark' ? 'light' : 'dark';
        applyTheme(nextTheme);
        localStorage.setItem(STORAGE_KEY, nextTheme);

        if (!localStorage.getItem('tiarun-gallery-theme')) {
          setGalleryTheme(nextTheme, false);
        }
      });
    }

    window.matchMedia('(prefers-color-scheme: dark)').addEventListener('change', (e) => {
      if (!localStorage.getItem(STORAGE_KEY)) {
        const systemTheme = e.matches ? 'dark' : 'light';
        applyTheme(systemTheme);
        if (!localStorage.getItem('tiarun-gallery-theme')) {
          setGalleryTheme(systemTheme, false);
        }
      }
    });
  }

  /* ==========================================================================
     2. Gallery Segmented Control & Preview Theme
     ========================================================================== */
  function setGalleryTheme(theme, save = true) {
    const gallery = document.getElementById('gallery');
    const btnDark = document.getElementById('btn-theme-dark');
    const btnLight = document.getElementById('btn-theme-light');
    if (!gallery) return;

    gallery.setAttribute('data-gallery-theme', theme);

    if (btnDark && btnLight) {
      if (theme === 'dark') {
        btnDark.classList.add('active');
        btnDark.setAttribute('aria-pressed', 'true');
        btnLight.classList.remove('active');
        btnLight.setAttribute('aria-pressed', 'false');
      } else {
        btnLight.classList.add('active');
        btnLight.setAttribute('aria-pressed', 'true');
        btnDark.classList.remove('active');
        btnDark.setAttribute('aria-pressed', 'false');
      }
    }

    if (save) {
      localStorage.setItem('tiarun-gallery-theme', theme);
    }
  }

  function initGalleryTheme() {
    const btnDark = document.getElementById('btn-theme-dark');
    const btnLight = document.getElementById('btn-theme-light');

    const savedGalleryTheme = localStorage.getItem('tiarun-gallery-theme');
    const currentSiteTheme = document.documentElement.getAttribute('data-theme') || 'dark';
    const initialTheme = savedGalleryTheme || currentSiteTheme;

    setGalleryTheme(initialTheme, false);

    if (btnDark) {
      btnDark.addEventListener('click', () => setGalleryTheme('dark', true));
    }
    if (btnLight) {
      btnLight.addEventListener('click', () => setGalleryTheme('light', true));
    }
  }

  /* ==========================================================================
     3. Gallery Tabs Scroll & Keyboard Navigation
     ========================================================================== */
  function initGalleryTabs() {
    const tabRadios = document.querySelectorAll('.tab-radio');
    const tabLabels = document.querySelectorAll('.tab-label');

    tabRadios.forEach((radio) => {
      radio.addEventListener('change', () => {
        if (!radio.checked) return;
        const matchingLabel = document.querySelector(`label[for="${radio.id}"]`);
        if (matchingLabel) {
          matchingLabel.scrollIntoView({ behavior: 'smooth', block: 'nearest', inline: 'center' });
        }
      });
    });

    tabLabels.forEach((label) => {
      label.addEventListener('keydown', (e) => {
        if (e.key === 'Enter' || e.key === ' ') {
          e.preventDefault();
          const targetId = label.getAttribute('for');
          const targetRadio = document.getElementById(targetId);
          if (targetRadio) {
            targetRadio.checked = true;
            targetRadio.dispatchEvent(new Event('change'));
          }
        }
      });
    });
  }

  /* ==========================================================================
     4. Usage Interactive Commands (Click to Inspect on Gallery)
     ========================================================================== */
  function initCommandInspector() {
    const commandRows = document.querySelectorAll('.command-row[data-target-tab]');
    const galleryEl = document.getElementById('gallery');

    function activateTab(tabKey) {
      const radio = document.getElementById(`example-${tabKey}`);
      if (!radio) return;

      radio.checked = true;
      radio.dispatchEvent(new Event('change'));

      if (galleryEl) {
        galleryEl.scrollIntoView({ behavior: 'smooth', block: 'center' });
      }
    }

    commandRows.forEach((row) => {
      const tabKey = row.dataset.targetTab;
      if (!tabKey) return;

      row.addEventListener('click', () => activateTab(tabKey));

      row.addEventListener('keydown', (e) => {
        if (e.key === 'Enter' || e.key === ' ') {
          e.preventDefault();
          activateTab(tabKey);
        }
      });
    });
  }

  /* ==========================================================================
     5. Multi-Layer Client OS Detection & Auto-Adaptation
     ========================================================================== */
  function detectClientOS() {
    // 1. User-Agent Client Hints API (Chrome 90+, Edge, Brave, Opera)
    if (navigator.userAgentData && navigator.userAgentData.platform) {
      const p = navigator.userAgentData.platform.toLowerCase();
      if (p.includes('win')) return 'windows';
      if (p.includes('linux')) return 'linux';
      if (p.includes('mac')) return 'mac';
    }

    // 2. Navigator platform & userAgent strings (Firefox, Safari, Linux desktop browsers)
    const platform = (navigator.platform || '').toLowerCase();
    const ua = (navigator.userAgent || '').toLowerCase();

    if (platform.includes('win') || ua.includes('windows') || ua.includes('win32') || ua.includes('win64')) {
      return 'windows';
    }
    if (platform.includes('linux') || platform.includes('x11') || ua.includes('linux') || ua.includes('x11') || ua.includes('ubuntu')) {
      return 'linux';
    }
    if (platform.includes('mac') || ua.includes('macintosh') || ua.includes('mac os')) {
      return 'mac';
    }

    return null;
  }

  function applyOSAdaptation() {
    currentDetectedOS = detectClientOS();

    const badgeWin = document.getElementById('badge-windows');
    const cardWin = document.getElementById('card-windows');
    const badgeLinux = document.getElementById('badge-linux');
    const cardLinux = document.getElementById('card-linux');

    // 1. Highlight appropriate download card
    if (currentDetectedOS === 'windows') {
      if (badgeWin) badgeWin.hidden = false;
      if (cardWin) cardWin.classList.add('highlighted-os');
      if (badgeLinux) badgeLinux.hidden = true;
      if (cardLinux) cardLinux.classList.remove('highlighted-os');
    } else if (currentDetectedOS === 'linux') {
      if (badgeLinux) badgeLinux.hidden = false;
      if (cardLinux) cardLinux.classList.add('highlighted-os');
      if (badgeWin) badgeWin.hidden = true;
      if (cardWin) cardWin.classList.remove('highlighted-os');
    }

    // 2. Update Hero CTA and hint based on detected OS
    updateHeroCTA();
  }

  function updateHeroCTA() {
    const heroBtn = document.querySelector('.hero-download-btn');
    const heroBtnText = document.querySelector('.hero-download-text');
    const heroHint = document.getElementById('hero-os-hint');

    if (!heroBtn || !heroBtnText) return;

    if (currentDetectedOS === 'linux') {
      heroBtnText.textContent = `Tải cho Linux (.deb) · ${currentLatestVersion}`;
      if (directDownloadUrls.deb) {
        heroBtn.href = directDownloadUrls.deb;
      } else {
        heroBtn.href = '#card-linux';
      }
      if (heroHint) {
        heroHint.textContent = 'Phát hiện máy bạn dùng Linux (x86-64). Cũng có sẵn cho Windows.';
        heroHint.hidden = false;
      }
    } else if (currentDetectedOS === 'windows') {
      heroBtnText.textContent = `Tải cho Windows (.exe) · ${currentLatestVersion}`;
      if (directDownloadUrls.exe) {
        heroBtn.href = directDownloadUrls.exe;
      } else {
        heroBtn.href = '#card-windows';
      }
      if (heroHint) {
        heroHint.textContent = 'Phát hiện máy bạn dùng Windows (x86-64). Cũng có sẵn cho Linux.';
        heroHint.hidden = false;
      }
    } else {
      heroBtnText.textContent = `Tải TiaRun ${currentLatestVersion}`;
      heroBtn.href = '#download';
      if (heroHint) heroHint.hidden = true;
    }
  }

  /* ==========================================================================
     6. GitHub Releases API & Dynamic Assets Resolution
     ========================================================================== */
  function formatBytes(bytes) {
    if (!bytes || isNaN(bytes)) return '';
    const mb = bytes / (1024 * 1024);
    return `${mb.toFixed(1)} MB`;
  }

  async function resolveReleaseData() {
    const packageRules = {
      exe: {
        pattern: /^tiarun-setup\.exe$/,
        sizeElemId: 'size-exe',
        formatSize: (s) => `· ${s}`,
      },
      zip: {
        pattern: /^tiarun-windows-x86_64\.zip$/,
        sizeElemId: 'size-zip',
        formatSize: (s) => `(${s})`,
      },
      deb: {
        pattern: /^tiarun_[\w.+~-]+_amd64\.deb$/,
        sizeElemId: 'size-deb',
        formatSize: (s) => `· ${s}`,
      },
      tar: {
        pattern: /^tiarun-linux-x86_64\.tar\.gz$/,
        sizeElemId: 'size-tar',
        formatSize: (s) => `(${s})`,
      },
    };

    try {
      const response = await fetch(`https://api.github.com/repos/${GITHUB_REPO}/releases/latest`);
      if (!response.ok) return;

      const release = await response.json();
      if (!release) return;

      // 1. Update Version Badges
      const versionTag = release.tag_name || release.name;
      if (versionTag) {
        currentLatestVersion = versionTag;
        const badge = document.getElementById('release-version-badge');
        if (badge) badge.textContent = versionTag;
      }

      // 2. Match Release Assets
      if (Array.isArray(release.assets)) {
        for (const [pkgKey, rule] of Object.entries(packageRules)) {
          const asset = release.assets.find(
            ({ name, browser_download_url: url }) =>
              rule.pattern.test(name) &&
              /^https:\/\/github\.com\/hieunx1024\/tiarun\/releases\/download\/[^/]+\/[^/]+$/.test(url)
          );

          if (!asset) continue;

          // Save direct download URL for Hero & Card links
          directDownloadUrls[pkgKey] = asset.browser_download_url;

          // Update download card links
          const link = document.querySelector(`[data-package="${pkgKey}"]`);
          if (link) {
            link.href = asset.browser_download_url;
          }

          // Update dynamic file size display
          if (rule.sizeElemId && asset.size) {
            const sizeElem = document.getElementById(rule.sizeElemId);
            if (sizeElem) {
              const formatted = formatBytes(asset.size);
              sizeElem.textContent = rule.formatSize(formatted);
            }
          }
        }
      }

      // Re-apply Hero CTA with resolved version & direct URLs
      updateHeroCTA();
    } catch (err) {
      // Graceful degradation: static default links remain functional if GitHub API is offline or rate-limited.
    }
  }

  /* ==========================================================================
     DOM Ready Initialization
     ========================================================================== */
  document.addEventListener('DOMContentLoaded', () => {
    initSiteTheme();
    initGalleryTheme();
    initGalleryTabs();
    initCommandInspector();
    applyOSAdaptation();
    resolveReleaseData();
  });
})();
