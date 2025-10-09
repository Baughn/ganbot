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
                const modelConfig = JSON.parse(galleryCell.dataset.modelConfig || 'null');
                const prompt = galleryCell.dataset.prompt || '';
                const img = link.querySelector('img');
                const imgAlt = img.alt;
                showModal(modal, urls, imgAlt, modelConfig, prompt);
            } else {
                // Fallback for non-gallery cells
                const img = link.querySelector('img');
                const imgSrc = img.src;
                const imgAlt = img.alt;
                showModal(modal, [imgSrc], imgAlt, null, '');
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

        // Create left side container for info and prompt
        const leftContainer = document.createElement('div');
        leftContainer.className = 'image-modal-left';

        // Create model info panel
        const infoPanel = document.createElement('div');
        infoPanel.className = 'image-modal-info';

        // Create prompt panel
        const promptPanel = document.createElement('div');
        promptPanel.className = 'image-modal-prompt';

        // Create image grid container (right side)
        const imageGrid = document.createElement('div');
        imageGrid.className = 'image-modal-grid';

        // Assemble modal
        leftContainer.appendChild(infoPanel);
        leftContainer.appendChild(promptPanel);
        container.appendChild(closeBtn);
        container.appendChild(leftContainer);
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

        return { overlay, infoPanel, promptPanel, imageGrid };
    }

    function showModal(modal, imageUrls, imgAlt, modelConfig, prompt) {
        // Clear existing content
        modal.imageGrid.innerHTML = '';
        modal.infoPanel.innerHTML = '';
        modal.promptPanel.innerHTML = '';

        // Populate info panel if model config is available
        if (modelConfig) {
            const infoHTML = formatModelConfig(modelConfig);
            modal.infoPanel.innerHTML = infoHTML;
        }

        // Populate prompt panel if prompt is available
        if (prompt) {
            const promptHTML = `<h3>Prompt</h3><p class="prompt-text">${prompt}</p>`;
            modal.promptPanel.innerHTML = promptHTML;
        }

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

    function formatModelConfig(config) {
        let html = `<h2>${config.name}</h2>`;

        if (config.description) {
            html += `<p class="model-description">${config.description}</p>`;
        }

        html += '<div class="model-specs">';

        if (config.checkpoint) {
            // ComfyUI model
            html += `<div class="spec-group">`;
            html += `<h3>Model Configuration</h3>`;
            html += `<div class="spec-item"><span class="spec-label">Checkpoint:</span> <span class="spec-value">${config.checkpoint}</span></div>`;
            html += `<div class="spec-item"><span class="spec-label">Resolution:</span> <span class="spec-value">${config.resolution}</span></div>`;
            html += `</div>`;

            html += `<div class="spec-group">`;
            html += `<h3>Sampling</h3>`;
            html += `<div class="spec-item"><span class="spec-label">Steps:</span> <span class="spec-value">${config.steps}</span></div>`;
            html += `<div class="spec-item"><span class="spec-label">CFG:</span> <span class="spec-value">${config.cfg}</span></div>`;
            html += `<div class="spec-item"><span class="spec-label">Sampler:</span> <span class="spec-value">${config.sampler}</span></div>`;
            html += `<div class="spec-item"><span class="spec-label">Scheduler:</span> <span class="spec-value">${config.scheduler}</span></div>`;
            html += `</div>`;

            // Two-stage upscaling if present
            html += `<div class="spec-group">`;
            if (config.two_stage) {
                html += `<h3>Two-Stage Upscaling Enabled</h3>`;
                if (config.upscale_factor) {
                    html += `<div class="spec-item"><span class="spec-label">Upscale Factor:</span> <span class="spec-value">${config.upscale_factor}x</span></div>`;
                }
                if (config.stage2_denoise) {
                    html += `<div class="spec-item"><span class="spec-label">Stage 2 Denoise:</span> <span class="spec-value">${config.stage2_denoise}</span></div>`;
                }
                if (config.stage2_sampler) {
                    html += `<div class="spec-item"><span class="spec-label">Stage 2 Sampler:</span> <span class="spec-value">${config.stage2_sampler}</span></div>`;
                }
                if (config.stage2_scheduler) {
                    html += `<div class="spec-item"><span class="spec-label">Stage 2 Scheduler:</span> <span class="spec-value">${config.stage2_scheduler}</span></div>`;
                }
            } else {
                html += `<h3>Two-Stage Upscaling Disabled</h3>`;
            }
            html += `</div>`;
        } else if (config.backend) {
            // NanoBanana model
            html += `<div class="spec-group">`;
            html += `<div class="spec-item"><span class="spec-label">Backend:</span> <span class="spec-value">${config.backend}</span></div>`;
            html += `</div>`;
        }

        html += '</div>';
        return html;
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
