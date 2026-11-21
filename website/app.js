const repository = 'hieunx1024/tiarun';
const packages = {
  exe: { pattern: /^(?:tiarun|view-launcher)-setup\.exe$/, label: 'Tải bộ cài (.exe)' },
  zip: { pattern: /^(?:tiarun|view-launcher)-windows-x86_64\.zip$/ },
  deb: { pattern: /^(?:tiarun|view-launcher)_[\w.+~-]+_amd64\.deb$/, label: 'Tải gói Debian / Ubuntu (.deb)' },
  tar: { pattern: /^(?:tiarun|view-launcher)-linux-x86_64\.tar\.gz$/ },
};

async function resolveDownloads() {
  try {
    const response = await fetch(`https://api.github.com/repos/${repository}/releases/latest`);
    if (!response.ok) return;

    const release = await response.json();
    if (!Array.isArray(release.assets)) return;

    for (const link of document.querySelectorAll('[data-package]')) {
      const { pattern, label } = packages[link.dataset.package];
      const asset = release.assets.find(({ name, browser_download_url: url }) =>
        pattern.test(name) &&
        /^https:\/\/github\.com\/hieunx1024\/(?:tiarun|view-launcher)\/releases\/download\/[^/]+\/[^/]+$/.test(url),
      );
      if (!asset) continue;

      link.href = asset.browser_download_url;
      if (label) link.firstChild.textContent = `${label} `;
      link.closest('li').hidden = false;
    }
  } catch {
    // The release page remains usable if GitHub's API is unavailable.
  }
}

resolveDownloads();

function initGalleryTheme() {
  const gallery = document.getElementById('gallery');
  const toggleBtn = document.getElementById('theme-switch-btn');
  if (!gallery || !toggleBtn) return;

  const savedTheme = localStorage.getItem('tiarun-gallery-theme') || 'dark';
  gallery.setAttribute('data-theme', savedTheme);

  toggleBtn.addEventListener('click', () => {
    const currentTheme = gallery.getAttribute('data-theme') || 'dark';
    const nextTheme = currentTheme === 'dark' ? 'light' : 'dark';
    gallery.setAttribute('data-theme', nextTheme);
    localStorage.setItem('tiarun-gallery-theme', nextTheme);
  });
}

initGalleryTheme();
