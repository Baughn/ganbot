// Gallery tag and style filtering functionality
document.addEventListener('DOMContentLoaded', () => {
    const filterButtons = document.querySelectorAll('.filter[data-tag]');
    const styleFilterButtons = document.querySelectorAll('.filter[data-style]');
    const modelRows = document.querySelectorAll('tbody tr[data-tags]');

    // Get tag from URL parameter
    function getTagFromURL() {
        const params = new URLSearchParams(window.location.search);
        return params.get('tag');
    }

    // Get style from URL parameter
    function getStyleFromURL() {
        const params = new URLSearchParams(window.location.search);
        return params.get('style');
    }

    // Get modal state from URL parameters
    function getModalFromURL() {
        const params = new URLSearchParams(window.location.search);
        return {
            active: params.get('modal') === 'true',
            model: params.get('model'),
            prompt: params.get('prompt')
        };
    }

    // Generic URL update function that preserves other parameters
    function updateURL(params) {
        const url = new URL(window.location);
        for (const [key, value] of Object.entries(params)) {
            if (value === null || value === undefined) {
                url.searchParams.delete(key);
            } else {
                url.searchParams.set(key, value);
            }
        }
        window.history.pushState({}, '', url);
    }

    // Wrapper for updating tag parameter
    function updateTagURL(tag) {
        updateURL({ tag: tag });
    }

    // Wrapper for updating style parameter
    function updateStyleURL(style) {
        updateURL({ style: style });
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

    // Set active style button (styles don't filter, they reload the page)
    function setActiveStyle(style) {
        // Update active button
        styleFilterButtons.forEach(btn => {
            if (btn.dataset.style === style) {
                btn.classList.add('active');
            } else {
                btn.classList.remove('active');
            }
        });
    }

    // Initialize: read tag from URL or use active button
    const urlTag = getTagFromURL();
    if (urlTag) {
        // URL has a tag parameter, use it
        setActiveTag(urlTag);
    } else {
        // No URL parameter, use the server-set active button and update URL to match
        const activeButton = document.querySelector('.filter[data-tag].active');
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
            updateTagURL(selectedTag);
        });
    });

    // Initialize style from URL
    const urlStyle = getStyleFromURL();
    if (urlStyle) {
        setActiveStyle(urlStyle);
    } else {
        // Use server-set default and update URL
        const activeStyleButton = document.querySelector('.filter[data-style].active');
        if (activeStyleButton) {
            const serverDefaultStyle = activeStyleButton.dataset.style;
            const url = new URL(window.location);
            url.searchParams.set('style', serverDefaultStyle);
            window.history.replaceState({}, '', url);
        }
    }

    // Handle style filter button clicks (reloads the page with new style)
    styleFilterButtons.forEach(button => {
        button.addEventListener('click', () => {
            const selectedStyle = button.dataset.style;

            // Update URL and reload
            updateStyleURL(selectedStyle);
            window.location.reload();
        });
    });

    // Handle browser back/forward navigation
    window.addEventListener('popstate', () => {
        const currentTag = getTagFromURL();
        if (currentTag) {
            setActiveTag(currentTag);
        }
        // For style changes, we need to reload since images are different
        const currentStyle = getStyleFromURL();
        const activeStyleButton = document.querySelector('.filter[data-style].active');
        if (activeStyleButton && currentStyle && activeStyleButton.dataset.style !== currentStyle) {
            window.location.reload();
        }

        // Handle modal state changes (needs modal to be created first)
        // This will be set up after modal creation below
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

    // Auto-open modal if URL contains modal parameters
    const modalState = getModalFromURL();
    if (modalState.active && modalState.model && modalState.prompt) {
        // Find the gallery cell matching the model and prompt
        const galleryCells = document.querySelectorAll('.gallery-cell');
        for (const cell of galleryCells) {
            const cellModelConfig = JSON.parse(cell.dataset.modelConfig || 'null');
            const cellPrompt = cell.dataset.prompt || '';

            if (cellModelConfig && cellModelConfig.name === modalState.model && cellPrompt === modalState.prompt) {
                const urls = JSON.parse(cell.dataset.urls || '[]');
                const link = cell.querySelector('.gallery-link');
                if (link) {
                    const img = link.querySelector('img');
                    const imgAlt = img ? img.alt : '';
                    showModal(modal, urls, imgAlt, cellModelConfig, cellPrompt);
                }
                break;
            }
        }
    }

    // Handle browser back/forward for modal state
    window.addEventListener('popstate', () => {
        const currentModalState = getModalFromURL();
        const modalIsOpen = modal.overlay.classList.contains('active');

        if (currentModalState.active && !modalIsOpen && currentModalState.model && currentModalState.prompt) {
            // URL says modal should be open, but it's closed - open it
            const galleryCells = document.querySelectorAll('.gallery-cell');
            for (const cell of galleryCells) {
                const cellModelConfig = JSON.parse(cell.dataset.modelConfig || 'null');
                const cellPrompt = cell.dataset.prompt || '';

                if (cellModelConfig && cellModelConfig.name === currentModalState.model && cellPrompt === currentModalState.prompt) {
                    const urls = JSON.parse(cell.dataset.urls || '[]');
                    const link = cell.querySelector('.gallery-link');
                    if (link) {
                        const img = link.querySelector('img');
                        const imgAlt = img ? img.alt : '';
                        showModal(modal, urls, imgAlt, cellModelConfig, cellPrompt, true);
                    }
                    break;
                }
            }
        } else if (!currentModalState.active && modalIsOpen) {
            // URL says modal should be closed, but it's open - close it
            hideModal(modal.overlay, true);
        }
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

        // Create regen button container
        const regenContainer = document.createElement('div');
        regenContainer.className = 'image-modal-regen';
        const regenBtn = document.createElement('button');
        regenBtn.className = 'regen-btn';
        regenBtn.textContent = 'Regenerate Images';
        regenBtn.setAttribute('aria-label', 'Regenerate this set of 4 images');
        const regenStatus = document.createElement('div');
        regenStatus.className = 'regen-status';
        regenContainer.appendChild(regenBtn);
        regenContainer.appendChild(regenStatus);

        // Create image grid container (right side)
        const imageGrid = document.createElement('div');
        imageGrid.className = 'image-modal-grid';

        // Assemble modal
        leftContainer.appendChild(infoPanel);
        leftContainer.appendChild(promptPanel);
        leftContainer.appendChild(regenContainer);
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

        // Regen button click handler
        regenBtn.addEventListener('click', async () => {
            const modelName = regenBtn.dataset.modelName;
            const prompt = regenBtn.dataset.prompt;

            if (!modelName || !prompt) {
                console.error('Cannot regenerate: missing model or prompt information');
                regenStatus.textContent = 'Error: Missing model or prompt information';
                regenStatus.className = 'regen-status error';
                return;
            }

            // Extract style from URL
            const urlParams = new URLSearchParams(window.location.search);
            const styleName = urlParams.get('style') || 'default';

            // Show loading state
            regenBtn.disabled = true;
            regenBtn.textContent = 'Regenerating...';
            regenBtn.classList.add('loading');
            regenStatus.textContent = 'Generating 4 new images...';
            regenStatus.className = 'regen-status info';

            try {
                const response = await fetch('/gallery/regen', {
                    method: 'POST',
                    headers: {
                        'Content-Type': 'application/json',
                    },
                    body: JSON.stringify({
                        model_name: modelName,
                        prompt: prompt,
                        style_name: styleName,
                    }),
                });

                // Get raw response text for better error diagnostics
                const responseText = await response.text();
                console.log('Regen response status:', response.status);
                console.log('Regen response body:', responseText);

                // Try to parse as JSON
                let data;
                try {
                    data = JSON.parse(responseText);
                } catch {
                    const preview = responseText.substring(0, 200);
                    throw new Error(`Server returned non-JSON response (status ${response.status}): ${preview}...`);
                }

                if (!response.ok || !data.success) {
                    throw new Error(data.error || `HTTP ${response.status}`);
                }

                // Update status
                regenStatus.textContent = 'Updating gallery...';

                // Find and update the gallery cell
                const galleryCells = document.querySelectorAll('.gallery-cell');
                for (const cell of galleryCells) {
                    const cellModelConfig = JSON.parse(cell.dataset.modelConfig || 'null');
                    const cellPrompt = cell.dataset.prompt || '';

                    if (cellModelConfig && cellModelConfig.name === modelName && cellPrompt === prompt) {
                        // Update the cell's URLs
                        cell.dataset.urls = JSON.stringify(data.urls);

                        // Update all image elements in the cell
                        const links = cell.querySelectorAll('.gallery-link');
                        data.urls.forEach((url, index) => {
                            if (links[index]) {
                                const img = links[index].querySelector('img');
                                if (img) {
                                    img.src = url;
                                    // Force reload
                                    const cacheBuster = `?t=${Date.now()}`;
                                    img.src = url + cacheBuster;
                                }
                                links[index].href = url;
                            }
                        });

                        break;
                    }
                }

                // Show success message
                regenStatus.textContent = 'Success! Images regenerated.';
                regenStatus.className = 'regen-status success';
                console.log('Images regenerated successfully!');

                // Close modal after a short delay
                setTimeout(() => {
                    hideModal(overlay);
                }, 1500);
            } catch (error) {
                console.error('Regeneration failed:', error);
                regenStatus.textContent = `Error: ${error.message}`;
                regenStatus.className = 'regen-status error';
                // Reset button state on error
                regenBtn.disabled = false;
                regenBtn.textContent = 'Regenerate Images';
                regenBtn.classList.remove('loading');
            }
        });

        return { overlay, infoPanel, promptPanel, imageGrid, regenBtn, regenContainer, regenStatus };
    }

    function getFullResolutionUrl(thumbnailUrl) {
        // Convert thumbnail URL to full resolution
        // Example: /image/0.5/75/{uuid}.jpg -> /image/1.0/90/{uuid}.jpg
        const parts = thumbnailUrl.split('/');
        if (parts.length >= 5 && parts[0] === '' && parts[1] === 'image') {
            // Replace scale (index 2) and quality (index 3), keep uuid.jpg (index 4)
            parts[2] = '1.0';  // full size
            parts[3] = '90';   // high quality
            return parts.join('/');
        }
        // Fallback for non-compressed URLs (e.g., placeholders)
        return thumbnailUrl;
    }

    function showModal(modalComponents, imageUrls, imgAlt, modelConfig, prompt, skipUrlUpdate = false) {
        // Clear existing content
        modalComponents.imageGrid.innerHTML = '';
        modalComponents.infoPanel.innerHTML = '';
        modalComponents.promptPanel.innerHTML = '';

        // Populate info panel if model config is available
        if (modelConfig) {
            const infoHTML = formatModelConfig(modelConfig);
            modalComponents.infoPanel.innerHTML = infoHTML;
        }

        // Populate prompt panel if prompt is available
        if (prompt) {
            const promptHTML = `<h3>Prompt</h3><p class="prompt-text">${prompt}</p>`;
            modalComponents.promptPanel.innerHTML = promptHTML;
        }

        // Show/hide regen button based on config
        if (window.galleryConfig && window.galleryConfig.enableRegen && modelConfig) {
            modalComponents.regenContainer.style.display = 'block';
            // Store data needed for regeneration
            modalComponents.regenBtn.dataset.modelName = modelConfig.name;
            modalComponents.regenBtn.dataset.prompt = prompt || '';
            modalComponents.regenBtn.dataset.styleUrl = window.location.search; // Contains style param
            // Clear previous status
            modalComponents.regenStatus.textContent = '';
            modalComponents.regenStatus.className = 'regen-status';
            // Reset button state
            modalComponents.regenBtn.disabled = false;
            modalComponents.regenBtn.textContent = 'Regenerate Images';
            modalComponents.regenBtn.classList.remove('loading');
        } else {
            modalComponents.regenContainer.style.display = 'none';
        }

        // Add all images to the grid at full resolution
        imageUrls.forEach((url, index) => {
            const img = document.createElement('img');
            img.className = 'image-modal-img';
            img.src = getFullResolutionUrl(url);  // Convert to full-res URL
            img.alt = `${imgAlt} - ${index + 1}`;
            modalComponents.imageGrid.appendChild(img);
        });

        modalComponents.overlay.classList.add('active');
        document.body.style.overflow = 'hidden';

        // Update URL with modal parameters if we have model config (unless skipping)
        if (!skipUrlUpdate && modelConfig && modelConfig.name && prompt) {
            updateURL({
                modal: 'true',
                model: modelConfig.name,
                prompt: prompt
            });
        }
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

    function hideModal(overlay, skipUrlUpdate = false) {
        overlay.classList.remove('active');
        document.body.style.overflow = '';

        // Remove modal parameters from URL (unless skipping)
        if (!skipUrlUpdate) {
            updateURL({
                modal: null,
                model: null,
                prompt: null
            });
        }
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
                    const previousIndex = parseInt(cell.dataset.previousIndex);
                    const hasPrevious = !isNaN(previousIndex);

                    // Update visibility using z-index crossfade to prevent white flash
                    const links = cell.querySelectorAll('.gallery-link');
                    links.forEach((link, idx) => {
                        if (idx === imageIndex) {
                            // New image: behind at full opacity, skip transition
                            link.style.transition = 'none';
                            link.style.zIndex = '1';
                            link.style.opacity = '1';
                            link.style.pointerEvents = 'auto';
                            // Force reflow to apply transition: none
                            link.offsetHeight;
                            link.style.transition = '';
                        } else if (hasPrevious && idx === previousIndex) {
                            // Old image: on top, fading out with transition
                            link.style.transition = '';
                            link.style.zIndex = '2';
                            link.style.opacity = '0';
                            link.style.pointerEvents = 'none';
                        } else {
                            // Other images: hidden behind
                            link.style.zIndex = '1';
                            link.style.opacity = '0';
                            link.style.pointerEvents = 'none';
                        }
                    });

                    // Track current image for next cycle
                    cell.dataset.previousIndex = imageIndex;
                }
            });
        }, 250);
    }

    // Gallery pagination functionality
    const paginationTopContainer = document.querySelector('.gallery-pagination-top');
    const paginationBottomContainer = document.querySelector('.gallery-pagination-bottom');
    const comparisonGrid = document.querySelector('.comparison-grid');

    if (paginationTopContainer && comparisonGrid) {
        // Get all column headers (excluding the first "Model" column)
        const columnHeaders = Array.from(comparisonGrid.querySelectorAll('thead th[data-column-index]'));
        const totalColumns = columnHeaders.length;

        if (totalColumns === 0) {
            // No columns to paginate
            return;
        }

        // Pagination state
        let leftmostColumn = 0;
        let columnsPerPage = 1;

        // Calculate how many columns fit in the viewport
        function calculateColumnsPerPage() {
            const viewportWidth = window.innerWidth;
            const sidebarWidth = 200; // From CSS
            const modelColumnWidth = 150; // Approximate width of model name column
            const cellWidth = 216; // 200px image + 16px padding (from CSS)
            const scrollbarBuffer = 20; // Account for scrollbar
            const contentPadding = 64; // 2rem * 2 (left + right padding on .content)
            const extraBuffer = 10; // Additional safety margin

            const availableWidth = viewportWidth - sidebarWidth - modelColumnWidth - scrollbarBuffer - contentPadding - extraBuffer;
            const cols = Math.max(1, Math.floor(availableWidth / cellWidth));

            return cols;
        }

        // Get leftmost column from URL
        function getColumnFromURL() {
            const params = new URLSearchParams(window.location.search);
            const col = params.get('col');
            return col !== null ? parseInt(col, 10) : 0;
        }

        // Wrapper for updating column parameter
        function updateColumnURL(column) {
            updateURL({ col: column });
        }

        // Show/hide columns based on current leftmost column and columns per page
        function updateColumnVisibility() {
            const endColumn = Math.min(leftmostColumn + columnsPerPage, totalColumns);

            columnHeaders.forEach((header) => {
                const columnIndex = parseInt(header.dataset.columnIndex, 10);
                const isVisible = columnIndex >= leftmostColumn && columnIndex < endColumn;

                // Update header visibility
                header.style.display = isVisible ? '' : 'none';

                // Update all cells in this column
                const cells = comparisonGrid.querySelectorAll(`tbody td[data-column-index="${columnIndex}"]`);
                cells.forEach(cell => {
                    cell.style.display = isVisible ? '' : 'none';
                });
            });
        }

        // Render pagination controls
        function renderPagination() {
            paginationTopContainer.innerHTML = '';
            paginationTopContainer.appendChild(createPaginationControls());

            if (paginationBottomContainer) {
                paginationBottomContainer.innerHTML = '';
                paginationBottomContainer.appendChild(createPaginationControls());
            }
        }

        // Create pagination control elements
        function createPaginationControls() {
            const container = document.createElement('div');
            container.className = 'pagination-controls';

            // Previous button
            const prevBtn = document.createElement('button');
            prevBtn.className = 'pagination-btn pagination-prev';
            prevBtn.textContent = '‹ Prev';
            prevBtn.disabled = leftmostColumn === 0;
            prevBtn.addEventListener('click', () => {
                // Jump to previous aligned page boundary
                leftmostColumn = Math.max(0, Math.floor((leftmostColumn - 1) / columnsPerPage) * columnsPerPage);
                updateColumnURL(leftmostColumn);
                updateColumnVisibility();
                renderPagination();
            });
            container.appendChild(prevBtn);

            // Page buttons
            const pageButtonsContainer = document.createElement('div');
            pageButtonsContainer.className = 'pagination-pages';

            // Calculate natural page boundaries (multiples of columnsPerPage)
            const totalPages = Math.ceil(totalColumns / columnsPerPage);
            const currentPageIndex = Math.floor(leftmostColumn / columnsPerPage);
            const isAligned = leftmostColumn % columnsPerPage === 0;

            for (let page = 0; page < totalPages; page++) {
                const pageStartColumn = page * columnsPerPage;

                // If we're on an unaligned position and this is where we'd insert the indicator
                if (!isAligned && page === currentPageIndex + 1) {
                    const intermediateBtn = document.createElement('button');
                    intermediateBtn.className = 'pagination-btn pagination-page pagination-intermediate active';
                    intermediateBtn.textContent = '•';
                    intermediateBtn.title = `Columns ${leftmostColumn + 1}-${Math.min(leftmostColumn + columnsPerPage, totalColumns)}`;
                    pageButtonsContainer.appendChild(intermediateBtn);
                }

                const pageBtn = document.createElement('button');
                pageBtn.className = 'pagination-btn pagination-page';
                pageBtn.textContent = (page + 1).toString();

                // Mark as active if this is the current aligned page
                if (isAligned && page === currentPageIndex) {
                    pageBtn.classList.add('active');
                }

                pageBtn.addEventListener('click', () => {
                    leftmostColumn = pageStartColumn;
                    updateColumnURL(leftmostColumn);
                    updateColumnVisibility();
                    renderPagination();
                });

                pageButtonsContainer.appendChild(pageBtn);
            }

            container.appendChild(pageButtonsContainer);

            // Next button
            const nextBtn = document.createElement('button');
            nextBtn.className = 'pagination-btn pagination-next';
            nextBtn.textContent = 'Next ›';
            // Check if next aligned page exists
            nextBtn.disabled = (Math.floor(leftmostColumn / columnsPerPage) + 1) * columnsPerPage >= totalColumns;
            nextBtn.addEventListener('click', () => {
                // Jump to next aligned page boundary
                leftmostColumn = (Math.floor(leftmostColumn / columnsPerPage) + 1) * columnsPerPage;
                updateColumnURL(leftmostColumn);
                updateColumnVisibility();
                renderPagination();
            });
            container.appendChild(nextBtn);

            return container;
        }

        // Initialize pagination
        function initializePagination() {
            columnsPerPage = calculateColumnsPerPage();
            leftmostColumn = getColumnFromURL();

            // Clamp leftmost column to valid range
            leftmostColumn = Math.max(0, Math.min(leftmostColumn, totalColumns - 1));

            updateColumnVisibility();
            renderPagination();
        }

        // Handle window resize - keep leftmost column anchored
        let resizeTimeout;
        window.addEventListener('resize', () => {
            clearTimeout(resizeTimeout);
            resizeTimeout = setTimeout(() => {
                const newColumnsPerPage = calculateColumnsPerPage();
                if (newColumnsPerPage !== columnsPerPage) {
                    columnsPerPage = newColumnsPerPage;
                    // Keep leftmost column anchored, just update visibility and controls
                    updateColumnVisibility();
                    renderPagination();
                }
            }, 250);
        });

        // Handle browser back/forward navigation
        window.addEventListener('popstate', () => {
            leftmostColumn = getColumnFromURL();
            leftmostColumn = Math.max(0, Math.min(leftmostColumn, totalColumns - 1));
            updateColumnVisibility();
            renderPagination();
        });

        // Initialize
        initializePagination();
    }
});
