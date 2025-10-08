// Gallery tag filtering functionality
document.addEventListener('DOMContentLoaded', () => {
    const filterButtons = document.querySelectorAll('.tag-filter');
    const modelRows = document.querySelectorAll('tbody tr[data-tags]');

    // Apply initial filter based on active button
    const activeButton = document.querySelector('.tag-filter.active');
    if (activeButton) {
        applyFilter(activeButton.dataset.tag);
    }

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

    filterButtons.forEach(button => {
        button.addEventListener('click', () => {
            const selectedTag = button.dataset.tag;

            // Update active button
            filterButtons.forEach(btn => btn.classList.remove('active'));
            button.classList.add('active');

            // Filter rows
            applyFilter(selectedTag);
        });
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
