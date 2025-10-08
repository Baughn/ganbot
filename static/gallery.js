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

            // Get the parent gallery cell to access all 4 images
            const galleryCell = link.closest('.gallery-cell');
            if (galleryCell) {
                const urls = JSON.parse(galleryCell.dataset.urls || '[]');
                const img = link.querySelector('img');
                const imgAlt = img.alt;
                showModal(modal, urls, imgAlt);
            } else {
                // Fallback for non-gallery cells
                const img = link.querySelector('img');
                const imgSrc = img.src;
                const imgAlt = img.alt;
                showModal(modal, [imgSrc], imgAlt);
            }
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

        // Create image grid container
        const imageGrid = document.createElement('div');
        imageGrid.className = 'image-modal-grid';

        // Assemble modal
        container.appendChild(closeBtn);
        container.appendChild(imageGrid);
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

        return { overlay, imageGrid };
    }

    function showModal(modal, imageUrls, imgAlt) {
        // Clear existing images
        modal.imageGrid.innerHTML = '';

        // Add all images to the grid
        imageUrls.forEach((url, index) => {
            const img = document.createElement('img');
            img.className = 'image-modal-img';
            img.src = url;
            img.alt = `${imgAlt} - ${index + 1}`;
            modal.imageGrid.appendChild(img);
        });

        modal.overlay.classList.add('active');
        document.body.style.overflow = 'hidden';
    }

    function hideModal(overlay) {
        overlay.classList.remove('active');
        document.body.style.overflow = '';
    }

    // Image cycling functionality for gallery cells
    const galleryCells = document.querySelectorAll('.gallery-cell');

    if (galleryCells.length > 0) {
        // Update image cycling every 250ms for smooth transitions
        setInterval(() => {
            const currentTime = Date.now();
            // Convert to quarter-seconds (250ms units)
            const quarterSeconds = Math.floor(currentTime / 250);

            galleryCells.forEach(cell => {
                const offset = parseInt(cell.dataset.cycleOffset) || 0;
                const urls = JSON.parse(cell.dataset.urls || '[]');

                if (urls.length === 4) {
                    // Calculate which image to show: (time + offset) / 40 % 4
                    // 40 quarter-seconds = 10 seconds = 0.1Hz period
                    const imageIndex = Math.floor((quarterSeconds + offset) / 40) % 4;

                    // Update visibility of links using opacity and pointer-events
                    const links = cell.querySelectorAll('.gallery-link');
                    links.forEach((link, idx) => {
                        if (idx === imageIndex) {
                            link.style.opacity = '1';
                            link.style.pointerEvents = 'auto';
                        } else {
                            link.style.opacity = '0';
                            link.style.pointerEvents = 'none';
                        }
                    });
                }
            });
        }, 250);
    }
});
