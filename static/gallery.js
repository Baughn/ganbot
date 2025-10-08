// Gallery tag filtering functionality
document.addEventListener('DOMContentLoaded', () => {
    const filterButtons = document.querySelectorAll('.tag-filter');
    const modelRows = document.querySelectorAll('tbody tr[data-tags]');

    // Get tag from URL parameter
    function getTagFromURL() {
        const params = new URLSearchParams(window.location.search);
        return params.get('tag');
    }

    // Update URL with selected tag
    function updateURL(tag) {
        const url = new URL(window.location);
        url.searchParams.set('tag', tag);
        window.history.pushState({}, '', url);
    }

    // Apply filter based on selected tag
    function applyFilter(selectedTag) {
        modelRows.forEach(row => {
            const rowTags = row.dataset.tags.split(',');

            if (selectedTag === 'all' || rowTags.includes(selectedTag)) {
                row.style.display = '';
            } else {
                row.style.display = 'none';
            }
        });
    }

    // Set active button and apply filter
    function setActiveTag(tag) {
        // Update active button
        filterButtons.forEach(btn => {
            if (btn.dataset.tag === tag) {
                btn.classList.add('active');
            } else {
                btn.classList.remove('active');
            }
        });

        // Apply filter
        applyFilter(tag);
    }

    // Initialize: read tag from URL or use active button
    const urlTag = getTagFromURL();
    if (urlTag) {
        // URL has a tag parameter, use it
        setActiveTag(urlTag);
    } else {
        // No URL parameter, use the server-set active button and update URL to match
        const activeButton = document.querySelector('.tag-filter.active');
        if (activeButton) {
            const serverDefaultTag = activeButton.dataset.tag;
            applyFilter(serverDefaultTag);
            // Update URL to reflect the server's default choice
            const url = new URL(window.location);
            url.searchParams.set('tag', serverDefaultTag);
            window.history.replaceState({}, '', url);
        }
    }

    // Handle filter button clicks
    filterButtons.forEach(button => {
        button.addEventListener('click', () => {
            const selectedTag = button.dataset.tag;

            // Update active state and filter
            setActiveTag(selectedTag);

            // Update URL
            updateURL(selectedTag);
        });
    });

    // Handle browser back/forward navigation
    window.addEventListener('popstate', () => {
        const urlTag = getTagFromURL();
        if (urlTag) {
            setActiveTag(urlTag);
        }
    });

    // Image zoom modal functionality
    const modal = createModal();
    const galleryLinks = document.querySelectorAll('.gallery-link');

    galleryLinks.forEach(link => {
        link.addEventListener('click', (e) => {
            e.preventDefault();
            const img = link.querySelector('img');
            const imgSrc = img.src;
            const imgAlt = img.alt;
            showModal(modal, imgSrc, imgAlt);
        });
    });

    function createModal() {
        // Create modal overlay
        const overlay = document.createElement('div');
        overlay.className = 'image-modal-overlay';

        // Create modal container
        const container = document.createElement('div');
        container.className = 'image-modal-container';

        // Create close button
        const closeBtn = document.createElement('button');
        closeBtn.className = 'image-modal-close';
        closeBtn.innerHTML = '&times;';
        closeBtn.setAttribute('aria-label', 'Close');

        // Create image element
        const img = document.createElement('img');
        img.className = 'image-modal-img';

        // Assemble modal
        container.appendChild(closeBtn);
        container.appendChild(img);
        overlay.appendChild(container);
        document.body.appendChild(overlay);

        // Close handlers
        closeBtn.addEventListener('click', () => hideModal(overlay));
        overlay.addEventListener('click', (e) => {
            if (e.target === overlay) {
                hideModal(overlay);
            }
        });

        document.addEventListener('keydown', (e) => {
            if (e.key === 'Escape' && overlay.classList.contains('active')) {
                hideModal(overlay);
            }
        });

        return { overlay, img };
    }

    function showModal(modal, imgSrc, imgAlt) {
        modal.img.src = imgSrc;
        modal.img.alt = imgAlt;
        modal.overlay.classList.add('active');
        document.body.style.overflow = 'hidden';
    }

    function hideModal(overlay) {
        overlay.classList.remove('active');
        document.body.style.overflow = '';
    }
});
