'use strict';

const REPO_URL = 'https://github.com/hieunx1024/tiarun';
const SITE_THEME_KEY = 'tiarun-site-theme';
const GALLERY_THEME_KEY = 'tiarun-gallery-theme';
const IS_EN = document.documentElement.lang.toLowerCase().startsWith('en');
const ASSET_PREFIX = document.documentElement.dataset.assetPrefix || 'assets/';

function readTheme(key) {
  try {
    const theme = localStorage.getItem(key);
    return theme === 'light' || theme === 'dark' ? theme : null;
  } catch {
    return null;
  }
}

function saveTheme(key, theme) {
  try {
    localStorage.setItem(key, theme);
  } catch {
    // Keep the selection for this page when the browser blocks storage.
  }
}

function initThemes() {
  const root = document.documentElement;
  const toggle = document.querySelector('#theme-toggle');
  const preference = window.matchMedia('(prefers-color-scheme: dark)');
  let chosenTheme = readTheme(SITE_THEME_KEY);
  const systemTheme = () => preference.matches ? 'dark' : 'light';

  function applyTheme(theme) {
    root.dataset.theme = theme;
    const label = IS_EN
      ? (theme === 'dark' ? 'Switch page to light theme' : 'Switch page to dark theme')
      : (theme === 'dark' ? 'Chuyển trang sang giao diện sáng' : 'Chuyển trang sang giao diện tối');
    toggle.setAttribute('aria-label', label);
    toggle.title = label;
  }

  applyTheme(chosenTheme || systemTheme());

  const gallery = initGallery();
  toggle.hidden = false;

  function applyPageTheme(theme) {
    applyTheme(theme);
    gallery.followPageTheme();
  }

  toggle.addEventListener('click', () => {
    chosenTheme = root.dataset.theme === 'dark' ? 'light' : 'dark';
    saveTheme(SITE_THEME_KEY, chosenTheme);
    applyPageTheme(chosenTheme);
  });

  preference.addEventListener('change', () => {
    if (!chosenTheme) applyPageTheme(systemTheme());
  });
}

function initGallery() {
  const tablist = document.querySelector('.gallery-tabs');
  const tabs = [...tablist.querySelectorAll('[role="tab"]')];
  const panels = tabs.map((tab) => document.getElementById(tab.getAttribute('aria-controls')));
  const themeOptions = document.querySelector('.theme-options');
  const themeButtons = [...themeOptions.querySelectorAll('button')];
  const compact = window.matchMedia('(max-width: 48rem)');
  let activeIndex = 0;
  let chosenTheme = readTheme(GALLERY_THEME_KEY);

  function updateImage() {
    const theme = chosenTheme || document.documentElement.dataset.theme;
    const panel = panels[activeIndex];
    const key = panel.id.slice('panel-'.length);
    const image = panel.querySelector('img');
    const source = `${ASSET_PREFIX}demo-${key}-${theme}.webp`;
    if (image.getAttribute('src') !== source) image.src = source;

    themeButtons.forEach((button) => {
      button.setAttribute('aria-pressed', String(button.dataset.galleryTheme === theme));
    });
  }

  function activate(index, moveFocus) {
    tabs[activeIndex].setAttribute('aria-selected', 'false');
    tabs[activeIndex].tabIndex = -1;
    panels[activeIndex].hidden = true;

    activeIndex = index;
    const tab = tabs[index];
    tab.setAttribute('aria-selected', 'true');
    tab.tabIndex = 0;
    updateImage();
    panels[index].hidden = false;
    if (moveFocus) tab.focus();
  }

  function updateOrientation() {
    tablist.setAttribute('aria-orientation', compact.matches ? 'horizontal' : 'vertical');
  }

  panels.forEach((panel, index) => {
    panel.setAttribute('role', 'tabpanel');
    panel.setAttribute('aria-labelledby', tabs[index].id);
    panel.tabIndex = 0;
  });

  tabs.forEach((tab, index) => tab.addEventListener('click', () => activate(index, false)));

  tablist.addEventListener('keydown', (event) => {
    const previousKey = compact.matches ? 'ArrowLeft' : 'ArrowUp';
    const nextKey = compact.matches ? 'ArrowRight' : 'ArrowDown';
    const lastIndex = tabs.length - 1;
    let targetIndex = activeIndex;

    switch (event.key) {
      case 'Home': targetIndex = 0; break;
      case 'End': targetIndex = lastIndex; break;
      case previousKey: targetIndex = activeIndex === 0 ? lastIndex : activeIndex - 1; break;
      case nextKey: targetIndex = activeIndex === lastIndex ? 0 : activeIndex + 1; break;
      default: return;
    }

    event.preventDefault();
    activate(targetIndex, true);
  });

  themeButtons.forEach((button) => {
    button.addEventListener('click', () => {
      chosenTheme = button.dataset.galleryTheme;
      saveTheme(GALLERY_THEME_KEY, chosenTheme);
      updateImage();
    });
  });

  compact.addEventListener('change', updateOrientation);
  updateOrientation();
  updateImage();
  tablist.hidden = false;
  themeOptions.hidden = false;

  return { followPageTheme: updateImage };
}

function showPlatformDownload() {
  const agent = navigator.userAgent || '';
  const hints = navigator.userAgentData;
  const platform = hints?.platform || navigator.platform || '';
  if (hints?.mobile || /Android|iPhone|iPad|iPod|CrOS/i.test(agent)) return;

  let target;
  let label;
  if (/^win/i.test(platform) || /Windows NT/i.test(agent)) {
    target = 'card-windows';
    label = IS_EN ? 'Download for Windows' : 'Tải cho Windows';
  } else if (/linux|^x11/i.test(platform) || /X11;.*Linux/i.test(agent)) {
    target = 'card-linux';
    label = IS_EN ? 'Download for Linux' : 'Tải cho Linux';
  } else {
    return;
  }

  const link = document.querySelector('#hero-download');
  link.href = `#${target}`;
  link.querySelector('span').textContent = label;
  document.getElementById(target).classList.add('is-current-platform');
}

const packagePatterns = {
  exe: /^tiarun-setup\.exe$/,
  zip: /^tiarun-windows-x86_64\.zip$/,
  deb: /^tiarun_[\w.+~-]+_amd64\.deb$/,
  tar: /^tiarun-linux-x86_64\.tar\.gz$/,
};

const sizeFormat = new Intl.NumberFormat(IS_EN ? 'en-US' : 'vi-VN', { maximumFractionDigits: 1 });

async function updateDownloads() {
  try {
    const response = await fetch('https://api.github.com/repos/hieunx1024/tiarun/releases/latest');
    if (!response.ok) return;

    const release = await response.json();
    if (!release || !/^v?\d+\.\d+\.\d+(?:[.+-][\w.-]+)?$/.test(release.tag_name)) return;

    const tag = release.tag_name;
    const baseDownload = `${REPO_URL}/releases/download/${encodeURIComponent(tag)}/`;
    const version = document.querySelector('#release-version');
    version.textContent = tag;
    version.hidden = false;
    document.querySelector('#release-link').href = `${REPO_URL}/releases/tag/${encodeURIComponent(tag)}`;

    if (!Array.isArray(release.assets)) return;
    document.querySelectorAll('[data-package]').forEach((link) => {
      const pattern = packagePatterns[link.dataset.package];
      const asset = release.assets.find((item) =>
        item && pattern.test(item.name) &&
        item.browser_download_url === baseDownload + encodeURIComponent(item.name)
      );
      if (!asset) return;

      link.href = asset.browser_download_url;
      link.querySelector('span').textContent = link.dataset.downloadLabel;
      if (Number.isFinite(asset.size) && asset.size > 0) {
        const size = link.querySelector('small');
        size.textContent = `${sizeFormat.format(asset.size / 1024 ** 2)} MiB`;
        size.hidden = false;
      }
    });
  } catch {
    // The links in the HTML lead to the latest release if the API is unavailable.
  }
}

function initDonateDialog() {
  const dialog = document.querySelector('#donate-dialog');
  if (typeof dialog.showModal !== 'function') return;

  document.querySelector('#donate-link').addEventListener('click', (event) => {
    if (event.ctrlKey || event.metaKey || event.shiftKey || event.altKey) return;
    event.preventDefault();
    dialog.showModal();
  });

  dialog.addEventListener('click', (event) => {
    if (event.target !== dialog) return;
    const { left, right, top, bottom } = dialog.getBoundingClientRect();
    if (event.clientX < left || event.clientX > right || event.clientY < top || event.clientY > bottom) {
      dialog.close();
    }
  });
}

initThemes();
showPlatformDownload();
updateDownloads();
initDonateDialog();
